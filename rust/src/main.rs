#![feature(try_blocks)]

extern crate anyhow;
extern crate neovim_lib;
extern crate serde_json;
extern crate structopt;

use anyhow::{anyhow, Result};
use neovim_lib::{Neovim, RequestHandler, Session, Value};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use structopt::StructOpt;
use sylph::{lookup, Line, Match, Matcher};

fn to_value(m: Match) -> Value {
    Value::Map(vec![(Value::from("index"), Value::from(m.index))])
}

struct EventHandler {
    matcher: Matcher,
}

impl EventHandler {
    fn new() -> Self {
        EventHandler {
            matcher: Matcher::new(),
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
                    let matches = self
                        .matcher
                        .best_matches(query, context, num_matches, &lines)?;
                    Value::from(
                        matches
                            .into_iter()
                            .map(|m| to_value(m))
                            .collect::<Vec<Value>>(),
                    )
                }
                "selected" => {
                    let selected = Line::from_value(&args[0])?;
                    self.matcher.update(&selected.name);
                    Value::from(true)
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
struct Query<'a> {
    query: String,
    launched_from: String,
    #[serde(borrow)]
    lines: Vec<Line<'a>>,
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
                let matches = Matcher::new()
                    .best_matches(&json.query, &json.launched_from, 10, &json.lines)
                    .unwrap();
                println!("query: {}\n{:#?}", json.query, matches);
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
