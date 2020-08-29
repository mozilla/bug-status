use std::{collections::HashMap, error::Error, fmt::Display};

use serde_json::Value;

static BUGZILLA_URL: &str = "https://bugzilla.mozilla.org/rest/bug?whiteboard=[print2020_v81]&include_fields=id,summary,status,product,priority,attachments.content_type";

#[derive(Debug)]
struct State {
    name: String,
    landed: usize,
    in_review: usize,
    assigned: usize,
    p1: usize,
    p2: usize,
    p3: usize,
    other: usize,
}
impl State {
    fn new(name: &str) -> Self {
        State {
            name: name.to_owned(),
            landed: 0,
            in_review: 0,
            assigned: 0,
            p1: 0,
            p2: 0,
            p3: 0,
            other: 0,
        }
    }
}
impl Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        //  Front-end (81 only) - 41 patches landed, 12 patches up for review, 0 more bugs assigned.
        //    Unassigned: 7 P1s, 7 P2s, 4 P3s, 1 Unknown.
        f.write_fmt(format_args!(
            "  {} (81 only) - {} patches landed, {} patches up for review, {} more bugs assigned.
    Remaining: {} P1s, {} P2s, {} P3s, {} Unknown.",
            self.name,
            self.landed,
            self.in_review,
            self.assigned,
            self.p1,
            self.p2,
            self.p3,
            self.other
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
            if has_patch {
                state.in_review += 1;
            } else {
                match priority {
                    "P1" => state.p1 += 1,
                    "P2" => state.p2 += 1,
                    "P3" => state.p3 += 1,
                    _ => state.other += 1,
                }
            }
        }
        "NEW" | "UNCONFIRMED" => match priority {
            "P1" => state.p1 += 1,
            "P2" => state.p2 += 1,
            "P3" => state.p3 += 1,
            _ => state.other += 1,
        },
        _ => {
            println!("Unknown status!!!\n  {}\n  Bug {:?}\n", status, bug);
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut frontend_state: State = State::new("Front-end");
    let mut platform_state: State = State::new("Platform");
    let mut seen = vec![];

    let resp = reqwest::blocking::get(BUGZILLA_URL)?
        .json::<HashMap<String, Vec<HashMap<String, Value>>>>()?;
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
        if bug["product"].as_str().unwrap() == "Toolkit" {
            categorize_bug(bug, &mut frontend_state);
        } else {
            categorize_bug(bug, &mut platform_state);
        };
    }
    println!("{}\n{}", frontend_state, platform_state);
    Ok(())
}
