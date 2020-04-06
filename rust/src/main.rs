#![feature(try_blocks)]

extern crate anyhow;
extern crate itertools;
extern crate lazysort;
extern crate neovim_lib;
extern crate sublime_fuzzy;
use anyhow::{anyhow, Result};
use lazysort::SortedBy;
use neovim_lib::{Neovim, RequestHandler, Session, Value};

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
    num_matches: u64,
    lines: Vec<Line>,
) -> Result<Vec<Match>> {
    Ok(lines
        .iter()
        .filter_map(|line| {
            let ctx_score =
                sublime_fuzzy::best_match(context, &line.name).map_or(0., |m| m.score() as f64);
            if query.len() > 0 {
                sublime_fuzzy::best_match(query, &line.name).map(|m| Match {
                    path: line.path.clone(),
                    name: line.name.clone(),
                    score: m.score() as f64 + ctx_score,
                })
            } else {
                Some(Match {
                    path: line.path.clone(),
                    name: line.name.clone(),
                    score: ctx_score,
                })
            }
        })
        .sorted_by(|a, b| a.score.partial_cmp(&b.score).unwrap().reverse())
        .take(num_matches as usize)
        .collect())
}

struct EventHandler;

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
                    let matches = best_matches(query, context, num_matches, lines)?;
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

    let handler = EventHandler {};
    let receiver = nvim.session.start_event_loop_channel_handler(handler);
    for _ in receiver {}
}
