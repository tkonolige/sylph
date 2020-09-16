use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use mlua::prelude::*;
use mlua::{UserData, Value};
use std::thread;

use super::matcher::*;

#[derive(Debug)]
enum Command {
    Query {
        query: String,
        context: String,
        num_results: usize,
        lines: Vec<OwnedLine>,
        id: usize,
    },
    Update(String),
}

/// Object holding a matcher running in a separate thread
pub struct ThreadedMatcher {
    command_ch: Sender<Command>,
    result_ch: Receiver<(usize, Result<Vec<Match>>)>,
    command_num: usize,
}

impl ThreadedMatcher {
    fn new() -> Self {
        let (command_send, command_recv) = unbounded();
        let (result_send, result_recv) = unbounded::<(usize, Result<Vec<Match>>)>();
        thread::spawn(move || {
            let mut matcher = match Matcher::new() {
                Ok(matcher) => matcher,
                Err(err) => {
                    eprintln!("{}", err);
                    result_send.send((0, Err(err))).unwrap();
                    return;
                }
            };
            loop {
                match command_recv.recv().unwrap() {
                    Command::Query {
                        query,
                        context,
                        num_results,
                        lines,
                        id,
                    } => {
                        let r: Result<()> = try {
                            let mut inc_matcher = matcher.incremental_match(
                                &query,
                                &context,
                                num_results as u64,
                                lines.as_slice(),
                            );
                            let mut progress = Progress::Working;
                            while command_recv.len() == 0 && progress == Progress::Working {
                                progress = inc_matcher.process(100)?;
                            }
                            if let Progress::Done(results) = progress {
                                result_send.send((id, Ok(results))).unwrap();
                            }
                        };
                        if let Err(err) = r {
                            result_send.send((id, Err(err))).unwrap();
                        }
                    }
                    Command::Update(path) => matcher.update(&path).unwrap(),
                }
            }
        });
        ThreadedMatcher {
            command_ch: command_send,
            result_ch: result_recv,
            command_num: 0,
        }
    }

    fn query<L: Line>(&mut self, query: &str, context: &str, num_results: usize, lines: &[L]) {
        self.command_num += 1;
        self.command_ch
            .send(Command::Query {
                query: query.to_string(),
                context: context.to_string(),
                num_results,
                lines: lines
                    .iter()
                    .map(|l| OwnedLine {
                        path: l.path().to_string(),
                        line: l.line().to_string(),
                    })
                    .collect(),
                id: self.command_num,
            });
    }

    fn get_result(&self) -> Option<Result<Vec<Match>>> {
        match self.result_ch.try_recv() {
            Ok((id, result)) => match id {
                0 => Some(result),
                i if i < self.command_num => self.get_result(),
                i if i == self.command_num => Some(result),
                _ => unreachable!(),
            },
            Err(TryRecvError::Disconnected) => Some(Err(anyhow!("Processing thread has died"))),
            _ => None,
        }
    }

    fn update(&self, path: &str) {
        self.command_ch
            .send(Command::Update(path.to_string()))
            .unwrap();
    }
}

impl<'lua> FromLua<'lua> for OwnedLine {
    fn from_lua(value: Value<'lua>, _: &'lua Lua) -> mlua::Result<Self> {
        match value {
            Value::Table(tbl) => {
                let line = tbl.get("line")?;
                let path = match tbl.get("location")? {
                    Value::Table(loc_tbl) => loc_tbl.get("path"),
                    x => Err(mlua::Error::FromLuaConversionError {
                        from: x.type_name(),
                        to: "location",
                        message: Some("expected table".to_string()),
                    }),
                }?;
                Ok(OwnedLine { path, line })
            }
            _ => Err(mlua::Error::FromLuaConversionError {
                from: value.type_name(),
                to: "OwnedLine",
                message: Some("expected table".to_string()),
            }),
        }
    }
}

impl<'lua> ToLua<'lua> for Match {
    fn to_lua(self, lua: &'lua Lua) -> mlua::Result<Value<'lua>> {
        let x = vec![
            ("index", self.index.to_lua(lua)?),
            ("score", self.score.to_lua(lua)?),
            ("context_score", self.context_score.to_lua(lua)?),
            ("query_score", self.query_score.to_lua(lua)?),
            ("frequency_score", self.frequency_score.to_lua(lua)?),
        ];
        lua.create_table_from(x.into_iter()).map(|x| Value::Table(x))
    }
}

impl UserData for ThreadedMatcher {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method_mut("query", |_, this, vals| {
                let (query, context, num_results, lines): (String, String, usize, Vec<OwnedLine>) =
                    vals;
                this.query(&query, &context, num_results, &lines);
                Ok(())
            });
        methods.add_method("get_result", |lua, this, _: ()| match this.get_result() {
            None => Ok((Value::Nil, Value::Nil)),
            Some(Ok(mtchs)) => Ok((mtchs.to_lua(lua)?, Value::Nil)),
            Some(Err(err)) => Ok((Value::Nil, err.to_string().to_lua(lua)?)),
        });
        methods.add_method("update", |_, this, s| {
            let s: String = s;
            this.update(&s);
            Ok(())
        });
    }
}

fn threaded_matcher(_: &Lua, _: ()) -> LuaResult<ThreadedMatcher> {
    Ok(ThreadedMatcher::new())
}

#[lua_module]
fn filter(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;
    exports.set("threaded_matcher", lua.create_function(threaded_matcher)?)?;
    Ok(exports)
}
