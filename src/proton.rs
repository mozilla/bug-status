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
#[derive(Clone, Debug)]
struct JiraIssue {
    /** The JIRA key, FIDEFE-123 */
    key: String,
    /** The link to this issue in JIRA */
    id: String,
    status: String,
    points: Option<u64>,
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

        let points = fields
            .get("customfield_10014")
            .unwrap_or(&Value::Null)
            .as_f64()
            .map(|x| x as u64);

        Self {
            key,
            id,
            status,
            points,
        }
    }
}

#[derive(Clone, Debug)]
struct BugzillaJiraLink {
    bugzilla: String,
    jira: JiraIssue,
    cached: bool,
}

impl BugzillaJiraLink {
    pub fn new(jira: JiraIssue, cached_data: &Map<String, Value>) -> Self {
        let (bugzilla, cached) = if let Some(data) = cached_data.get(&jira.key) {
            (data.as_str().unwrap().to_owned(), false)
        } else {
            let link = format!(
                "https://jira.mozilla.com/rest/api/2/issue/{}/remotelink",
                &jira.key
            );
            let resp: Vec<HashMap<String, Value>> = get_link(&link, true).unwrap();
            let data = resp[0]["object"]
                .as_object()
                .unwrap_or_else(|| panic!("Could not get object from {}", link));

            (
                data["url"]
                    .as_str()
                    .unwrap_or_else(|| panic!("Could not get url from {}", link))
                    .replace("https://bugzilla.mozilla.org/show_bug.cgi?id=", ""),
                true,
            )
        };
        Self {
            bugzilla,
            jira,
            cached,
        }
    }
}

#[derive(Clone, Debug)]
struct BugzillaBug {
    id: String,
    status: String,
    points: Option<u64>,
    has_patch: bool,
    jira: JiraIssue,
}

impl BugzillaBug {
    pub fn new(link: BugzillaJiraLink, bz_statuses: &HashMap<String, Map<String, Value>>) -> Self {
        let id = link.bugzilla;
        let bz_data = bz_statuses.get(&id).unwrap();
        let status = bz_data
            .get("status")
            .unwrap_or_else(|| panic!("Could not get status from {:?}", bz_data))
            .as_str()
            .unwrap_or_else(|| panic!("Could not get status from {:?}", bz_data))
            .to_string();
        let points = bz_data
            .get("cf_fx_points")
            .unwrap_or_else(|| panic!("Could not get points from {:?}", bz_data))
            .as_str()
            .unwrap_or_else(|| panic!("Could not get points from {:?}", bz_data))
            .parse::<u64>()
            .ok();

        let has_patch = if let Some(attachments) = bz_data.get("attachments") {
            if let Some(attachments) = attachments.as_array() {
                attachments.iter().any(|attachment| {
                    if let Some(attachment) = attachment.as_object() {
                        if let Some(content_type) = attachment.get("content_type") {
                            if let Some(content_type) = content_type.as_str() {
                                content_type == "text/x-phabricator-request"
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        } else {
            false
        };

        let jira = link.jira;
        Self {
            id,
            status,
            points,
            has_patch,
            jira,
        }
    }

    pub fn get_jira_status(&self) -> String {
        match self.status.as_str() {
            "ASSIGNED" => {
                if self.has_patch {
                    "In Review".to_string()
                } else {
                    "In Progress".to_string()
                }
            }
            "NEW" | "UNCONFIRMED" | "REOPENED" => "Open".to_string(),
            "???" => "Unknown".to_string(),
            _ => self.status.clone(),
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

    // println!(
    //     "Open:\nhttps://bugzilla.mozilla.org/buglist.cgi?bug_id={}",
    //     bugs.iter()
    //         .filter_map(|item| {
    //             if item.jira.status != "Closed" {
    //                 Some(item.id.as_str())
    //             } else {
    //                 None
    //             }
    //         })
    //         .collect::<Vec<_>>()
    //         .join(",")
    // );

    let mut header1 = false;
    for bug in &bugs {
        if bug.points.is_some() && bug.points != bug.jira.points {
            if !header1 {
                println!("\n\nChanged points:");
                header1 = true;
            }
            println!("  https://bugzilla.mozilla.org/show_bug.cgi?id={} ({:?}) => https://jira.mozilla.com/browse/{} ({:?})",
                bug.id, bug.points, bug.jira.key, bug.jira.points);
        }
    }

    let mut header2 = false;
    for bug in &bugs {
        if bug.get_jira_status() != bug.jira.status {
            if !header2 {
                println!("\n\nChanged status:");
                header2 = true;
            }
            println!("  https://bugzilla.mozilla.org/show_bug.cgi?id={} ({:?}) => https://jira.mozilla.com/browse/{} ({:?})",
                bug.id, bug.get_jira_status(), bug.jira.key, bug.jira.status);
        }
    }

    if !header1 && !header2 {
        println!("\n\nNo changes necessary! 🎉\n");
    }
    Ok(())
}

fn get_link<T: for<'de> Deserialize<'de>>(link: &str, auth: bool) -> Result<T> {
    let mut request = reqwest::blocking::Client::new().get(link);
    if auth {
        request = request.basic_auth("bwinton@mozilla.com", Some(JIRA_PASSWORD.to_string()));
    }
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
    issues: Vec<JiraIssue>,
    cached_data: &mut Map<String, Value>,
) -> Result<Vec<BugzillaBug>> {
    let bar = ProgressBar::new(issues.len() as u64);
    bar.set_style(ProgressStyle::default_bar().template(
        "Getting links: {spinner:.green} [{elapsed_precise}] [{bar:50.cyan/blue}] ({pos}/{len}, ETA {eta})",
    ));

    let links: Vec<BugzillaJiraLink> = issues
        .into_par_iter()
        .map(|issue| {
            bar.inc(1);
            BugzillaJiraLink::new(issue, &cached_data)
        })
        .collect();
    bar.finish();

    let mut bz_statuses = HashMap::new();
    let bar = ProgressBar::new(links.len() as u64);
    bar.set_style(ProgressStyle::default_bar().template(
        "Getting bugs: {spinner:.green} [{elapsed_precise}] [{bar:50.cyan/blue}] ({pos}/{len}, ETA {eta})",
    ));
    for links in links.chunks(200) {
        let list = format!("https://bugzilla.mozilla.org/rest/bug?id={}&include_fields=id,summary,status,product,priority,attachments.content_type,cf_fx_points",
            links.iter().map(|x| x.bugzilla.as_str()).collect::<Vec<_>>().join(","));
        let bugs: HashMap<String, Value> = get_link(&list, true).unwrap();
        bar.inc(links.len() as u64);
        let bugs = bugs
            .get("bugs")
            .unwrap_or_else(|| panic!("Could not get bugs from {}", list))
            .as_array()
            .unwrap_or_else(|| panic!("Could not get bugs from {}", list));
        for bug in bugs {
            let bug = bug
                .as_object()
                .unwrap_or_else(|| panic!("Could not get bug from {}", &bug));
            let id = bug
                .get("id")
                .unwrap_or_else(|| panic!("Could not get id from {:?}", bug))
                .as_u64()
                .unwrap_or_else(|| panic!("Could not get id from {:?}", bug));
            bz_statuses.insert(id.to_string(), bug.clone());
        }
    }
    bar.finish();

    let bugs = links
        .into_iter()
        .filter_map(|link| {
            if !link.cached {
                let key = link.jira.key.clone();
                let value = Value::String(link.bugzilla.clone());
                cached_data.insert(key, value);
            }

            if link.bugzilla.is_empty() {
                None
            } else {
                Some(BugzillaBug::new(link, &bz_statuses))
            }
        })
        .collect::<Vec<_>>();
    // println!("bugs: {:#?}", &bugs[..10]);
    Ok(bugs)
}

fn get_list() -> Result<Vec<JiraIssue>> {
    // Get the list of issues first.
    let list =
        // Issues in a sprint: "https://jira.mozilla.com/rest/api/2/search?jql=sprint%3D1071&fields=none&maxResults=100";
        // Issues in the backlog: "https://jira.mozilla.com/rest/agile/1.0/board/694/backlog?fields=none&maxResults=1000";
        // All open issues:
        "https://jira.mozilla.com/rest/agile/1.0/board/694/issue?fields=none&maxResults=1000&jql=statusCategory%20!%3D%20Done";
    let issues: HashMap<String, Value> = get_link(&list, true).unwrap();
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
        "Getting issues: {spinner:.green} [{elapsed_precise}] [{bar:50.cyan/blue}] ({pos}/{len}, ETA {eta})",
    ));
    for issues in issues.chunks(200) {
        let list = format!("https://jira.mozilla.com/rest/api/2/search?jql=issueKey%20in%20({})&fields=status,customfield_10014&maxResults=1000",
            issues.join("%2C"));
        let issues: HashMap<String, Value> = get_link(&list, true).unwrap();
        bar.inc(issues.len() as u64);
        let issues = issues
            .get("issues")
            .unwrap_or_else(|| panic!("Could not get issues from {}", list))
            .as_array()
            .unwrap_or_else(|| panic!("Could not get issues from {}", list));

        for issue in issues {
            rv.push(JiraIssue::new(issue));
        }
    }
    bar.finish();
    Ok(rv)
}
