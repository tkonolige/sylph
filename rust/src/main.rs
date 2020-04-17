#![feature(try_blocks)]

extern crate anyhow;
extern crate fuzzy_matcher;
extern crate itertools;
extern crate neovim_lib;
extern crate serde;
extern crate serde_json;
extern crate structopt;
extern crate sublime_fuzzy;

use anyhow::{anyhow, Result};
use fuzzy_matcher::FuzzyMatcher;
use neovim_lib::{Neovim, RequestHandler, Session, Value};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use structopt::StructOpt;

fn lookup<'a>(val: &'a Value, key: &str) -> Result<&'a Value> {
    let map: &Vec<(Value, Value)> =
        val.as_map()
            .ok_or(anyhow!("{} is not a map. Cannot lookup key {}.", val, key))?;
    map.iter()
        .find(|x| x.0.as_str().map_or(false, |y| y == key))
        .ok_or(anyhow!(
            "Key {} not found in map. Possible keys: {}",
            key,
            map.iter()
                .map(|x| format!("{}", x.0))
                .collect::<Vec<_>>()
                .join(",")
        ))
        .map(|x| &x.1)
}

#[derive(Deserialize)]
struct Line {
    path: String,
    name: String,
}

impl Line {
    fn from_value(val: &Value) -> Result<Self> {
        let path = lookup(val, "path")?
            .as_str()
            .ok_or(anyhow!("Key path is not a string."))?;
        let name = lookup(val, "name")?
            .as_str()
            .ok_or(anyhow!("Key name is not a string."))?;
        Ok(Line {
            path: path.to_string(),
            name: name.to_string(),
        })
    }
}

#[derive(Debug)]
struct Match {
    path: String,
    name: String,
    score: f64,
}

impl Match {
    fn to_value(self) -> Value {
        Value::Map(vec![
            (Value::from("path"), Value::from(self.path)),
            (Value::from("name"), Value::from(self.name)),
            (Value::from("score"), Value::from(self.score)),
        ])
    }
}

fn best_matches(
    query: &str,
    context: &str,
    frequency: &FrequencyCounter,
    num_matches: u64,
    lines: Vec<Line>,
) -> Result<Vec<Match>> {
    let matcher = fuzzy_matcher::skim::SkimMatcherV2::default().use_cache(true);
    let mtchs = lines
        .into_iter()
        .filter_map(|line| {
            let frequency_score = frequency.score(context);
            // let ctx_score =
            //     sublime_fuzzy::best_match(context, &line.name).map_or(0., |m| m.score() as f64);
            let ctx_score = matcher.fuzzy_match(&line.name, context).unwrap_or(0) as f64;
            let query_score = if query.len() > 0 {
                matcher.fuzzy_match(&line.name, query).unwrap_or(0)
            } else {
                0
            } as f64;
            Some(Match {
                path: line.path,
                name: line.name,
                score: frequency_score + ctx_score + query_score,
            })
        })
        .fold(Vec::new(), |mut entries, mtch| {
            if entries.len() < num_matches as usize {
                entries.push(mtch);
                entries
            } else {
                let pos = entries.iter().position(|x| x.score < mtch.score);
                match pos {
                    Some(idx) => entries[idx] = mtch,
                    None => (),
                }
                entries
            }
        });
    // .collect::<Vec<Vec<_>>>();
    // let mut short_list = mtchs.into_iter().flatten().collect::<Vec<_>>();
    // short_list.sort_unstable_by(|a, b| {
    //     a.score
    //         .partial_cmp(&b.score)
    //         .unwrap_or(std::cmp::Ordering::Equal)
    //         .reverse()
    // });
    let short_list = mtchs;
    Ok(short_list
        .into_iter()
        .take(num_matches as usize)
        .collect::<Vec<_>>())
}

struct FrequencyCounter {
    counts: HashMap<String, isize>,
    total: isize,
}

impl FrequencyCounter {
    fn new() -> Self {
        FrequencyCounter {
            counts: HashMap::new(),
            total: 0,
        }
    }

    fn update(&mut self, entry: &str) {
        *self.counts.entry(entry.to_string()).or_insert(0) += 1;
        self.total += 1;
    }

    fn score(&self, entry: &str) -> f64 {
        if self.total == 0 {
            0.
        } else {
            *self.counts.get(entry).unwrap_or(&0) as f64 / self.total as f64
        }
    }
}

struct EventHandler {
    frequency: FrequencyCounter,
}

impl EventHandler {
    fn new() -> Self {
        EventHandler {
            frequency: FrequencyCounter::new(),
        }
    }
}

impl RequestHandler for EventHandler {
    fn handle_request(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> std::result::Result<Value, Value> {
        let result: Result<Value> = try {
            match name {
                "match" => {
                    let query = lookup(&args[0], "query")?
                        .as_str()
                        .ok_or(anyhow!("query argument is not a string"))?;
                    let context = lookup(&args[0], "context")?
                        .as_str()
                        .ok_or(anyhow!("context argument is not a string"))?;
                    let num_matches = lookup(&args[0], "num_matches")?
                        .as_u64()
                        .ok_or(anyhow!("num_matches argument is not an integer"))?;
                    let lines = itertools::process_results(
                        lookup(&args[0], "lines")?
                            .as_array()
                            .ok_or(anyhow!(
                                "lines argument {} is not an array",
                                lookup(&args[0], "lines").unwrap()
                            ))?
                            .iter()
                            .map(Line::from_value),
                        |iter| iter.collect::<Vec<_>>(),
                    )?;
                    if context.len() > 0 {
                        self.frequency.update(context);
                    }
                    let matches =
                        best_matches(query, context, &self.frequency, num_matches, lines)?;
                    Value::from(
                        matches
                            .into_iter()
                            .map(Match::to_value)
                            .collect::<Vec<Value>>(),
                    )
                }
                f => Err(anyhow!("No such function {}.", f))?,
            }
        };
        result.map_err(|err| Value::from(format!("{:?}", err)))
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "sylph", about = "Fuzzy finder for use with neovim")]
struct Opts {
    #[structopt(long = "test-file", parse(from_os_str))]
    test_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Query {
    query: String,
    launched_from: String,
    lines: Vec<Line>,
}

fn main() {
    let opt = Opts::from_args();
    match opt.test_file {
        Some(path) => {
            let file = File::open(path).unwrap();
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let l = line.unwrap();
                let sl = if &l[l.len() - 1..] == "\n" {
                    &l[..l.len() - 1]
                } else {
                    &l[..]
                };
                let json: Query = serde_json::from_str(sl).unwrap();
                let matches = best_matches(
                    &json.query,
                    &json.launched_from,
                    &FrequencyCounter::new(),
                    10,
                    json.lines,
                )
                .unwrap();
                println!("{:?}", matches);
            }
        }
        None => {
            let session = Session::new_parent().unwrap();
            let mut nvim = Neovim::new(session);

            let handler = EventHandler::new();
            let receiver = nvim.session.start_event_loop_channel_handler(handler);
            for _ in receiver {}
        }
    }
}
