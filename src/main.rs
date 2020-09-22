use std::{collections::HashMap, fmt::Display};

use color_eyre::eyre::Result;
use rayon::prelude::*;
use serde_json::Value;

#[derive(Debug)]
struct State {
    name: String,
    version: String,
    landed: usize,
    in_review: usize,
    assigned: usize,
    p1_left: usize,
    p2_left: usize,
    plower_left: usize,
    other_left: usize,
    p1_open: usize,
    p2_open: usize,
    plower_open: usize,
    other_open: usize,
}
impl State {
    fn new(name: &str, version: &str) -> Self {
        State {
            name: name.to_owned(),
            version: version.to_owned(),
            landed: 0,
            in_review: 0,
            assigned: 0,
            p1_left: 0,
            p2_left: 0,
            plower_left: 0,
            other_left: 0,
            p1_open: 0,
            p2_open: 0,
            plower_open: 0,
            other_open: 0,
        }
    }
    fn interesting(&self) -> bool {
        self.p1_open + self.p2_open + self.plower_open + self.other_open > 0
    }
}
impl Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        //  Front-end (81 only) - 41 patches landed, 12 patches up for review, 0 more bugs assigned.
        //    Unassigned: 7 P1s, 7 P2s, 4 P3s, 1 Unknown.
        f.write_fmt(format_args!(
            "  {} (v{}) - {} patches landed, {} patches up for review, {} more bugs assigned.
    Without Patches: {} P1s, {} P2s, {} P3-P5s, {} Unknown.
    Total Open:      {} P1s, {} P2s, {} P3-P5s, {} Unknown.",
            self.name,
            self.version,
            self.landed,
            self.in_review,
            self.assigned,
            self.p1_left,
            self.p2_left,
            self.plower_left,
            self.other_left,
            self.p1_open,
            self.p2_open,
            self.plower_open,
            self.other_open,
        ))
    }
}

fn categorize_bug(bug: &HashMap<String, Value>, state: &mut State) {
    let status = bug["status"].as_str().unwrap();
    let priority = bug["priority"].as_str().unwrap();
    let mut has_patch = false;
    let attachments = bug["attachments"].as_array().unwrap();
    for attachment in attachments {
        if attachment["content_type"].as_str().unwrap() == "text/x-phabricator-request" {
            has_patch = true;
        }
    }
    match status {
        "RESOLVED" | "VERIFIED" => {
            if has_patch {
                state.landed += 1;
            }
        }
        "ASSIGNED" => {
            match priority {
                "P1" => state.p1_open += 1,
                "P2" => state.p2_open += 1,
                "P3" | "P4" | "P5" => state.plower_open += 1,
                _ => state.other_open += 1,
            }
            if has_patch {
                state.in_review += 1;
            } else {
                match priority {
                    "P1" => state.p1_left += 1,
                    "P2" => state.p2_left += 1,
                    "P3" | "P4" | "P5" => state.plower_left += 1,
                    _ => state.other_left += 1,
                }
            }
        }
        "NEW" | "UNCONFIRMED" | "REOPENED" => match priority {
            "P1" => {
                state.p1_left += 1;
                state.p1_open += 1
            }
            "P2" => {
                state.p2_left += 1;
                state.p2_open += 1
            }
            "P3" | "P4" | "P5" => {
                state.plower_left += 1;
                state.plower_open += 1
            }
            _ => {
                state.other_left += 1;
                state.other_open += 1
            }
        },
        _ => {
            println!("Unknown status!!!\n  {}\n  Bug {:?}\n", status, bug);
        }
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let summary: Vec<(State,State)> = ["81", "82"].par_iter().map(|version| {
        let mut frontend_state: State = State::new("Front-end", version);
        let mut platform_state: State = State::new("Platform", version);
        let mut seen = vec![];

        let url = format!("https://bugzilla.mozilla.org/rest/bug?whiteboard=[print2020_v{}]&include_fields=id,summary,status,product,priority,attachments.content_type", version);
        // println!("Getting data for {}", url);
        let resp = reqwest::blocking::get(&url).unwrap_or_else(|_| panic!("Could not get data for {}", version))
            .json::<HashMap<String, Vec<HashMap<String, Value>>>>().unwrap_or_else(|_| panic!("Could not parse json for {}", version));
        let bugs = &resp["bugs"];

        // let raw = include_str!("../bug-data.json");
        // let data = serde_json::from_str::<HashMap<String, Vec<HashMap<String, Value>>>>(raw)?;
        // let bugs = &data["bugs"];

        for bug in bugs {
            let id = bug["id"].as_i64().unwrap();
            if seen.contains(&id) {
                println!("Duplicate bug!!!\n  {}\n  Bug {:?}\n", id, bug);
            }
            seen.push(id);
            match bug["product"].as_str().unwrap() {
                "Toolkit" | "Firefox" => {
                    categorize_bug(bug, &mut frontend_state);
                },
                _ => {
                    categorize_bug(bug, &mut platform_state);
                }
            };
        }
        (frontend_state, platform_state)
    }).collect();

    for (frontend, platform) in summary {
        if frontend.interesting() {
            println!("{}", frontend);
        }
        if platform.interesting() {
            println!("{}", platform);
        }
        if frontend.interesting() || platform.interesting() {
            println!();
        }
    }
    Ok(())
}
