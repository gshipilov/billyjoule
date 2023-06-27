use serenity::json::{json, Value};
use serenity::model::prelude::interaction::InteractionResponseType;
use serenity::prelude::*;
use serenity::model::application::interaction::application_command::{
    ApplicationCommandInteraction, CommandDataOption,
};
use serenity::model::application::interaction::autocomplete::*;
use serde::{Serialize, Deserialize};
use s3::creds::Credentials;
use s3::{Bucket, Region};
use crate::commands::err_response;
use meilisearch_sdk::client::Client as meili;
use meilisearch_sdk::client::*;
use anyhow::{Result, bail};

use std::env;

const VALID_EMOJI: &str = r#""#;

pub async fn do_emoji(ctx: &Context, command: ApplicationCommandInteraction) {
    let guild = match command.guild_id {
        Some(g) => g,
        None => {
            error!("No server associated with emoji invocation...?");
            return;
        }
    };
    let emoji_option: Vec<&CommandDataOption> = command
        .data
        .options
        .iter()
        .filter(|opt| opt.name == "emoji")
        .collect();
    let emoji_name = match &emoji_option[0].value {
        Some(s) => s,
        None => {
            error!("Did not receive an emoji name.");
            return;
        }
    };

    let s3_endpoint = env::var("EMOJI_S3_ENDPOINT").ok();
    let s3_bucket = env::var("EMOJI_S3_BUCKET").ok();
    if s3_endpoint.is_none() {
        err_response(&ctx, &command, "bot is misconfigured: missing s3 endpoint def").await;
        error!("need an s3 endpoint for emojis");
        return;
    }
    if s3_bucket.is_none() {
        err_response(&ctx, &command, "bot is misconfigured: missing s3 bucket def").await;
        error!("need a bucket name for emojis");
        return;
    }

    let bucket = Bucket::new(
        &s3_bucket.unwrap(),
        Region::Custom {
            region: "us-east-1".to_owned(),
            endpoint: s3_endpoint.unwrap(),
        },
        Credentials::default().unwrap(),
    )
        .unwrap()
        .with_path_style();

    let file_list = match bucket
        .list(
            format!("{}/", emoji_name.as_str().unwrap()),
            Some("/".to_owned()),
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            err_response(&ctx, &command, "couldn't list s3 contents. maybe wrong bucket or endpoint.").await;
            error!("{}", e);
            return;
        }
    };
    if file_list.len() == 0 {
        err_response(&ctx, &command, "couldn't list s3 contents. maybe wrong bucket or endpoint.").await;
        error!("emoji not found.");
        return;
    }
    if file_list[0].contents.len() == 0 {
        err_response(&ctx, &command, format!("emoji {} not found!",emoji_name).as_str()).await;
        error!("emoji not found.");
        return;
    }

    let image_data = match bucket.get_object(&file_list[0].contents[0].key).await {
        Ok(rs) => rs,
        Err(e) => {
            error!("Could not retrieve image from s3 bucket: {e}");
            return;
        }
    };

    let image_b64 = base64::encode(image_data.as_slice());
    let image_str = format!("data:image/png;base64,{}", image_b64);
    let emoji_name_sanitized = emoji_name.as_str().unwrap().replace("-", "_");
    match guild
        .create_emoji(&ctx.http, &emoji_name_sanitized, &image_str)
        .await
    {
        Ok(_) => {
            if let Err(e) = command
                .create_interaction_response(&ctx.http, |resp| {
                    resp.kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|message| {
                            message.content(format!(
                                "Emoji :{}: added to server.",
                                &emoji_name_sanitized
                            ))
                        })
                })
                .await
            {
                error!("Unable to send response to command: {}", e);
                return;
            }
        }
        Err(e) => {
            error!("Could not add emoji: {}", e);
            return;
        }
    }
}

pub async fn do_emoji_autocomplete(ctx: &Context, command: AutocompleteInteraction) {
    let emoji_option: Vec<&CommandDataOption> = command
        .data
        .options
        .iter()
        .filter(|opt| opt.name == "emoji")
        .collect();
    let emoji_name = match &emoji_option[0].value {
        Some(s) => s.as_str().unwrap(),
        None => {
            error!("Did not receive an emoji name.");
            return;
        }
    };
    let meili_server = env::var("MEILISEARCH_URL").ok();
    let meili_key = env::var("MEILISEARCH_KEY").ok();
    let mut results: Vec<Value> = vec![];
    if let Some(meili_url) = meili_server {
        info!("searching for results in meili");
        let client = meili::new(meili_url, meili_key);
        match client.index("emoji").search().with_query(&emoji_name).execute::<EmojiSearch>().await {
            Ok(s) => {
                s.hits.iter().for_each(|hit| {
                    let e = EmojiAutocompleteOption {
                        name: hit.result.name.clone(),
                        value: hit.result.name.clone()
                    };
                    match serde_json::to_value(&e) {
                        Ok(val) => {
                            results.push(val);
                        },
                    Err(e) => {
                        error!("Unable to convert results to proper format for autocomplete: {e}");
                    }};
                });

            },
            Err(e) => {
                error!("Unable to get results of search: {e}");
            }
        };
    } else {
        error!("can't search for results.");
    }
    let choices = match serde_json::to_value(&results) {
        Ok(s) => s,
        Err(e) => {
            error!("can't convert results to choice list.  Returning nothing for choices.");
            json!([])
        }
    };

    if let Err(e) = command
        .create_autocomplete_response(&ctx.http, |resp| {
            resp.set_choices(choices)
        }).await {
        error!("couldn't send autocomplete response: {e}");
        return;
    }
}


#[derive(Serialize,Deserialize,Debug)]
pub struct EmojiSearch {
    name: String
}

#[derive(Serialize,Deserialize,Debug)]
pub struct EmojiAutocompleteOption {
    name: String,
    value: String
}



pub async fn do_emoji_indexing(url: String) -> anyhow::Result<()> {
    let s3_endpoint = env::var("EMOJI_S3_ENDPOINT").ok();
    let s3_bucket = env::var("EMOJI_S3_BUCKET").ok();
    if s3_endpoint.is_none() {
        bail!("need an s3 endpoint for emojis");
    }
    if s3_bucket.is_none() {
        bail!("need a bucket name for emojis");

    }

    let bucket = Bucket::new(
        &s3_bucket.unwrap(),
        Region::Custom {
            region: "us-east-1".to_owned(),
            endpoint: s3_endpoint.unwrap(),
        },
        Credentials::default().unwrap(),
    )
        .unwrap()
        .with_path_style();

    let filenames = match get_emoji_directory_names(bucket).await {
        Some(f) => f,
        None => {
            bail!("No files found to index.");
        }
    };



    let client :meilisearch_sdk::client::Client = meili::new(url, None::<String>);
    let emoji = client.index("emoji");

    let mut search_data: Vec<EmojiSearch> = vec![];
    filenames.iter().for_each(|f| {
        search_data.push(EmojiSearch{name: f.to_string()});
    });
    debug!("PGPGPG About to add documents: {}", search_data.len());
    if let Err(e) =  emoji.add_documents(
        &search_data,
        Some("name")).await {
            bail!("Unable to index: {e}");
        };
    debug!("PGPGPG After add documents");
    Ok(())
}

async fn get_emoji_directory_names(bucket :Bucket) -> Option<Vec<String>> {

    let mut filenames: Vec<String> = vec![];
    debug!("Preparing to get file list from s3 bucket");
    let file_list = match bucket
        .list(
            String::default(),
            Some("/".to_owned()),
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            error!("{}", e);
            return None;
        }
    };
    if file_list.len() > 0 {
        let prefixes = file_list[0].clone().common_prefixes;
        if prefixes.is_some() {
            prefixes.unwrap().iter().for_each(|dir| {
                let dirname = dir.prefix.clone();
                filenames.push(dirname.strip_suffix("/").unwrap().to_string());
            });
        }
        Some(filenames)
    } else {
        None
    }
}