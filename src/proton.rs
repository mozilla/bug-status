use std::collections::HashMap;
use std::env::var;
use std::fs::{remove_file, File};
use std::path::Path;

use color_eyre::eyre::{eyre, Result};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use serde::Deserialize;
use serde_json::{from_reader, to_writer_pretty, Map, Value};

fn main() -> Result<()> {
    color_eyre::install()?;

    let cache_name = "jira.cache";
    let cache = Path::new(&cache_name);
    if !cache.is_file() {
        let cache_file = File::create(&cache_name)?;
        to_writer_pretty(cache_file, &Map::new())?;
    };
    let cache_file = File::open(&cache_name)?;
    let parsed_data = from_reader(cache_file);
    if parsed_data.is_err() {
        remove_file(cache)?;
    }
    let mut cached_data: Map<String, Value> = parsed_data?;
    println!("Found {} items in the cache.", cached_data.len());

    let jira_password = var("JIRA_PASSWORD")?;

    let bugs = get_list(&jira_password)?;
    let bugs = get_bugs(bugs, &mut cached_data, &jira_password)?;

    // `create` will also truncate an existing file.
    let cache_file = File::create(&cache_name)?;
    to_writer_pretty(cache_file, &cached_data)?;

    println!(
        "Priority:\n  \"bug_id\": \"{}\",",
        bugs.iter()
            .map(|item| { item.0.clone() })
            .collect::<Vec<_>>()
            .join(",")
    );
    println!(
        "Open:\nhttps://bugzilla.mozilla.org/buglist.cgi?bug_id={}",
        bugs.iter()
            .filter_map(|item| {
                if item.1 {
                    Some(item.0.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}

fn get_link<T: for<'de> Deserialize<'de>>(link: &str, jira_password: &str) -> Result<T> {
    let mut request = reqwest::blocking::Client::new().get(link);
    request = request.basic_auth("bwinton@mozilla.com", Some(jira_password));
    request = request.header(reqwest::header::CONTENT_TYPE, "application/json");
    let resp = request
        .send()
        .unwrap_or_else(|_| panic!("Could not get data for {}", link));
    if !resp.status().is_success() {
        return Err(eyre!("Got {} for {}", resp.status(), link));
    }
    let fields = resp
        .json::<T>()
        .unwrap_or_else(|_| panic!("Could not parse json from {}", link));
    Ok(fields)
}

type CachedData = Option<(String, Value)>;

fn get_bugs(
    bugs: Vec<Value>,
    cached_data: &mut Map<String, Value>,
    jira_password: &str,
) -> Result<Vec<(String, bool)>> {
    let bar = ProgressBar::new(bugs.len() as u64);
    bar.set_style(ProgressStyle::default_bar().template(
        "{spinner:.green} [{elapsed_precise}] [{bar:50.cyan/blue}] ({pos}/{len}, ETA {eta})",
    ));

    let bugs: Vec<(String, bool, CachedData)> = bugs
        .into_par_iter()
        .map(|item| {
            bar.inc(1);
            let key = item
                .get("key")
                .unwrap_or_else(|| panic!("Could not get key from {:?}", item))
                .as_str()
                .unwrap_or_else(|| panic!("Could not get key from {:?}", item));
            // let fields = item.get("fields").unwrap_or_else(|| panic!("Could not get fields from {}", key)).as_object().unwrap_or_else(|| panic!("Could not get fields from {}", key));
            // let status = fields.get("status").unwrap_or_else(|| panic!("Could not get status from {}", key)).as_object().unwrap_or_else(|| panic!("Could not get status from {}", key));
            // let base_status = status.get("name").unwrap_or_else(|| panic!("Could not get status name from {}", key)).as_str().unwrap_or_else(|| panic!("Could not get status name from {}", key));
            let me = item
                .get("self")
                .unwrap_or_else(|| panic!("Could not get self from {:?}", item))
                .as_str()
                .unwrap_or_else(|| panic!("Could not get self from {:?}", item));
            let item: HashMap<String, Value> =
                get_link(&format!("{}?fields=status", me), jira_password).unwrap();
            let fields = item
                .get("fields")
                .unwrap_or_else(|| panic!("Could not get fields from {}", key))
                .as_object()
                .unwrap_or_else(|| panic!("Could not get fields from {}", key));
            let status = fields
                .get("status")
                .unwrap_or_else(|| panic!("Could not get status from {}", key))
                .as_object()
                .unwrap_or_else(|| panic!("Could not get status from {}", key));
            let status = status
                .get("name")
                .unwrap_or_else(|| panic!("Could not get status name from {}", key))
                .as_str()
                .unwrap_or_else(|| panic!("Could not get status name from {}", key));

            if let Some(data) = cached_data.get(key) {
                (data.as_str().unwrap().to_owned(), status != "Closed", None)
            } else {
                let link = format!(
                    "https://jira.mozilla.com/rest/api/2/issue/{}/remotelink",
                    key
                );
                let resp: Vec<HashMap<String, Value>> = get_link(&link, jira_password).unwrap();
                let data = resp[0]["object"]
                    .as_object()
                    .unwrap_or_else(|| panic!("Could not get object from {}", link));

                (
                    data["url"]
                        .as_str()
                        .unwrap_or_else(|| panic!("Could not get url from {}", link))
                        .to_owned(),
                    status != "Closed",
                    Some((key.to_string(), data["url"].clone())),
                )
            }
        })
        .collect();
    bar.finish();

    let bugs = bugs
        .iter()
        .filter_map(|(url, status, insert)| {
            if let Some((key, value)) = insert.clone() {
                cached_data.insert(key, value);
            }
            if url.is_empty() {
                None
            } else {
                Some((
                    url.replace("https://bugzilla.mozilla.org/show_bug.cgi?id=", ""),
                    *status,
                ))
            }
        })
        .collect::<Vec<_>>();
    Ok(bugs)
}

fn get_list(jira_password: &str) -> Result<Vec<Value>> {
    // Get the list of issues first.
    let list =
        "https://jira.mozilla.com/rest/api/2/search?jql=sprint%3D1071&fields=none&maxResults=100";
    let issues: HashMap<String, Value> = get_link(&list, &jira_password).unwrap();
    let issues = issues
        .get("issues")
        .unwrap_or_else(|| panic!("Could not get issues from {}", list))
        .as_array()
        .unwrap_or_else(|| panic!("Could not get issues from {}", list));
    let issues = issues
        .iter()
        .map(|item| {
            item.get("key")
                .unwrap_or_else(|| panic!("Could not get key from {:?}", item))
                .as_str()
                .unwrap_or_else(|| panic!("Could not get key from {:?}", item))
        })
        .collect::<Vec<_>>();

    // Then get their statuses.
    let list = format!("https://jira.mozilla.com/rest/api/2/search?jql=issueKey%20in%20({})&fields=status&maxResults=100",
        issues.join("%2C"));
    let issues: HashMap<String, Value> = get_link(&list, &jira_password).unwrap();
    let issues = issues
        .get("issues")
        .unwrap_or_else(|| panic!("Could not get issues from {}", list))
        .as_array()
        .unwrap_or_else(|| panic!("Could not get issues from {}", list));
    Ok(issues.clone())
}
