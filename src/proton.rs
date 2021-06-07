use std::collections::HashMap;
use std::env::var;
use std::fs::{remove_file, File};
use std::path::Path;

use color_eyre::eyre::{eyre, Result};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use serde::Deserialize;
use serde_json::{from_reader, to_writer_pretty, Map, Value};

#[macro_use]
extern crate lazy_static;

lazy_static! {
    static ref JIRA_PASSWORD: String = {
        let password = var("JIRA_PASSWORD");
        if password.is_err() {
            panic!("Missing JIRA_PASSWORD.");
        }
        password.unwrap()
    };
}
#[derive(Clone)]
struct JiraIssue {
    key: String,
    id: String,
    status: String,
}

impl JiraIssue {
    pub fn new(item: &Value) -> Self {
        let key = item
            .get("key")
            .unwrap_or_else(|| panic!("Could not get key from {:?}", item))
            .as_str()
            .unwrap_or_else(|| panic!("Could not get key from {:?}", item))
            .to_string();

        let id = item
            .get("self")
            .unwrap_or_else(|| panic!("Could not get self from {:?}", item))
            .as_str()
            .unwrap_or_else(|| panic!("Could not get self from {:?}", item))
            .to_string();

        // let item: HashMap<String, Value> = get_link(&format!("{}?fields=status", id)).unwrap();
        let fields = item
            .get("fields")
            .unwrap_or_else(|| panic!("Could not get fields from {}", &key))
            .as_object()
            .unwrap_or_else(|| panic!("Could not get fields from {}", &key));
        let status = fields
            .get("status")
            .unwrap_or_else(|| panic!("Could not get status from {}", &key))
            .as_object()
            .unwrap_or_else(|| panic!("Could not get status from {}", &key));
        let status = status
            .get("name")
            .unwrap_or_else(|| panic!("Could not get status name from {}", &key))
            .as_str()
            .unwrap_or_else(|| panic!("Could not get status name from {}", &key))
            .to_string();

        Self { key, id, status }
    }
}

#[derive(Clone)]
struct BugzillaBug {
    link: String,
    issue: JiraIssue,
    cached: bool,
}

impl BugzillaBug {
    pub fn new(issue: JiraIssue, cached_data: &Map<String, Value>) -> Self {
        let (link, cached) = if let Some(data) = cached_data.get(&issue.key) {
            (data.as_str().unwrap().to_owned(), false)
        } else {
            let link = format!(
                "https://jira.mozilla.com/rest/api/2/issue/{}/remotelink",
                &issue.key
            );
            let resp: Vec<HashMap<String, Value>> = get_link(&link).unwrap();
            let data = resp[0]["object"]
                .as_object()
                .unwrap_or_else(|| panic!("Could not get object from {}", link));

            (
                data["url"]
                    .as_str()
                    .unwrap_or_else(|| panic!("Could not get url from {}", link))
                    .to_owned(),
                true,
            )
        };
        Self {
            link,
            issue,
            cached,
        }
    }
}

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

    let issues = get_list()?;
    let bugs = get_bugs(issues, &mut cached_data)?;

    // `create` will also truncate an existing file.
    let cache_file = File::create(&cache_name)?;
    to_writer_pretty(cache_file, &cached_data)?;

    println!(
        "Priority:\n  \"bug_id\": \"{}\",",
        bugs.iter()
            .map(|item| { item.link.as_str() })
            .collect::<Vec<_>>()
            .join(",")
    );
    println!(
        "Open:\nhttps://bugzilla.mozilla.org/buglist.cgi?bug_id={}",
        bugs.iter()
            .filter_map(|item| {
                if item.issue.status != "Closed" {
                    Some(item.link.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}

fn get_link<T: for<'de> Deserialize<'de>>(link: &str) -> Result<T> {
    let mut request = reqwest::blocking::Client::new().get(link);
    request = request.basic_auth("bwinton@mozilla.com", Some(JIRA_PASSWORD.to_string()));
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

fn get_bugs(
    bugs: Vec<JiraIssue>,
    cached_data: &mut Map<String, Value>,
) -> Result<Vec<BugzillaBug>> {
    let bar = ProgressBar::new(bugs.len() as u64);
    bar.set_style(ProgressStyle::default_bar().template(
        "{spinner:.green} [{elapsed_precise}] [{bar:50.cyan/blue}] ({pos}/{len}, ETA {eta})",
    ));

    let bugs: Vec<BugzillaBug> = bugs
        .into_par_iter()
        .map(|issue| {
            bar.inc(1);
            BugzillaBug::new(issue, &cached_data)
        })
        .collect();
    bar.finish();

    let bugs = bugs
        .into_iter()
        .filter_map(|mut bug| {
            if !bug.cached {
                let key = bug.issue.key.clone();
                let value = Value::String(bug.link.clone());
                cached_data.insert(key, value);
            }

            if bug.link.is_empty() {
                None
            } else {
                bug.link = bug
                    .link
                    .replace("https://bugzilla.mozilla.org/show_bug.cgi?id=", "");
                Some(bug)
            }
        })
        .collect::<Vec<_>>();
    Ok(bugs)
}

fn get_list() -> Result<Vec<JiraIssue>> {
    // Get the list of issues first.
    let list =
        // "https://jira.mozilla.com/rest/api/2/search?jql=sprint%3D1071&fields=none&maxResults=100";
        "https://jira.mozilla.com/rest/agile/1.0/board/694/backlog?fields=none&maxResults=1000";
    let issues: HashMap<String, Value> = get_link(&list).unwrap();
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
    let mut rv = vec![];
    let bar = ProgressBar::new(issues.len() as u64);
    bar.set_style(ProgressStyle::default_bar().template(
        "{spinner:.green} [{elapsed_precise}] [{bar:50.cyan/blue}] ({pos}/{len}, ETA {eta})",
    ));
    for issues in issues.chunks(50) {
        let list = format!("https://jira.mozilla.com/rest/api/2/search?jql=issueKey%20in%20({})&fields=status&maxResults=1000",
            issues.join("%2C"));
        let issues: HashMap<String, Value> = get_link(&list).unwrap();
        let issues = issues
            .get("issues")
            .unwrap_or_else(|| panic!("Could not get issues from {}", list))
            .as_array()
            .unwrap_or_else(|| panic!("Could not get issues from {}", list));
        for issue in issues {
            bar.inc(1);
            rv.push(JiraIssue::new(issue));
        }
    }
    bar.finish();
    Ok(rv)
}
