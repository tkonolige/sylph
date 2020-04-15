#![feature(try_blocks)]

extern crate anyhow;
extern crate itertools;
extern crate neovim_lib;
extern crate sublime_fuzzy;
extern crate rayon;

use anyhow::{anyhow, Result};
use neovim_lib::{Neovim, RequestHandler, Session, Value};
use std::collections::HashMap;
use rayon::prelude::*;
use rayon::iter::IntoParallelIterator;

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
    let mtchs = lines
        .into_par_iter()
        .filter_map(|line| {
            let frequency_score = frequency.score(context);
            let ctx_score =
                sublime_fuzzy::best_match(context, &line.name).map_or(0., |m| m.score() as f64);
            if query.len() > 0 {
                sublime_fuzzy::best_match(query, &line.name).map(|m| Match {
                    path: line.path,
                    name: line.name,
                    score: m.score() as f64 + ctx_score,
                })
            } else {
                Some(Match {
                    path: line.path,
                    name: line.name,
                    score: frequency_score + ctx_score,
                })
            }
        })
        .fold(|| Vec::new(), |mut entries, mtch| {
            if entries.len() < num_matches as usize {
                entries.push(mtch);
                entries
            } else {
                let pos = entries.iter().position(|x| x.score < mtch.score);
                match pos {
                    Some(idx) => entries[idx] = mtch,
                    None => ()
                }
                entries
            }
        })
        .collect::<Vec<Vec<_>>>();
    let mut short_list = mtchs.into_iter().flatten().collect::<Vec<_>>();
    short_list.sort_unstable_by(|a,b| a.score.partial_cmp(&b.score).unwrap().reverse());
    Ok(short_list.into_iter().take(num_matches as usize).collect::<Vec<_>>())
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
        *self.counts.get(entry).unwrap_or(&0) as f64 / self.total as f64
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

fn main() {
    let session = Session::new_parent().unwrap();
    let mut nvim = Neovim::new(session);

    let handler = EventHandler::new();
    let receiver = nvim.session.start_event_loop_channel_handler(handler);
    for _ in receiver {}
}
