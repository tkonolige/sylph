use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::thread;

use super::matcher::*;

#[repr(C)]
#[derive(Debug)]
pub struct RawLine {
    path: *const c_char,
    line: *const c_char,
}

impl Line for RawLine {
    fn path(&self) -> &str {
        unsafe { CStr::from_ptr(self.path).to_str().unwrap() }
    }
    fn line(&self) -> &str {
        unsafe { CStr::from_ptr(self.line).to_str().unwrap() }
    }
}

#[no_mangle]
pub extern "C" fn free_string(s: *mut c_char) {
    unsafe { CString::from_raw(s) };
}

fn return_c_error<A>(r: &Result<A>) -> *const c_char {
    match r {
        Ok(_) => std::ptr::null(),
        Err(err) => CString::new(format!("{}", err)).unwrap().into_raw(),
    }
}

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
                    .map(|l| OwnedLine{path: l.path().to_string(), line: l.line().to_string()})
                    .collect(),
                id: self.command_num,
            })
            .unwrap();
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

#[no_mangle]
pub extern "C" fn new_threaded_matcher(ptr: *mut *mut ThreadedMatcher) -> *const c_char {
    unsafe {
        *ptr = Box::into_raw(Box::new(ThreadedMatcher::new()));
    }
    return_c_error(&Ok(()))
}

#[no_mangle]
pub extern "C" fn free_threaded_matcher(matcher: *mut ThreadedMatcher) {
    unsafe { Box::from_raw(matcher) };
    ()
}

#[no_mangle]
pub extern "C" fn start_matches_threaded(
    matcher: *mut ThreadedMatcher,
    query: *const c_char,
    context: *const c_char,
    num_matches: u64,
    lines_ptr: *const RawLine,
    num_lines: u64,
) -> *const c_char {
    let res: Result<()> = try {
        // TODO: avoid allocating a vector?
        let lines = unsafe { std::slice::from_raw_parts(lines_ptr, num_lines as usize) };
        let q = unsafe { CStr::from_ptr(query).to_str()? };
        let c = unsafe { CStr::from_ptr(context).to_str()? };

        unsafe {
            matcher.as_mut().context("invalid pointer")?.query(
                q,
                c,
                num_matches as usize,
                lines.as_ref(),
            )
        }
    };
    return_c_error(&res)
}

#[no_mangle]
pub extern "C" fn get_matches_threaded(
    matcher: *const ThreadedMatcher,
    result_ptr: *mut Match,
    num_matches: *mut i64,
) -> *const c_char {
    let er = try {
        let matches = unsafe { matcher.as_ref().context("invalid pointer")?.get_result() };
        match matches {
            Some(m) => {
                let mtchs = m?;
                // copy matches into result vector
                let results_out =
                    unsafe { std::slice::from_raw_parts_mut(result_ptr, *num_matches as usize) };
                unsafe { *num_matches = mtchs.len() as i64 };
                for i in 0..mtchs.len() {
                    results_out[i] = mtchs[i].clone()
                }
            }
            None => unsafe {
                *num_matches = -1;
            },
        }
    };
    return_c_error(&er)
}

#[no_mangle]
pub extern "C" fn update_matcher_threaded(
    matcher: *mut ThreadedMatcher,
    path: *const c_char,
) -> *const c_char {
    println!("Updating");
    let r = unsafe {
        matcher
            .as_mut()
            .ok_or(anyhow!("Invalid matcher"))
            .and_then(|m| Ok(m.update(CStr::from_ptr(path).to_str()?)))
    };
    println!("done");
    return_c_error(&r)
}

#[no_mangle]
pub extern "C" fn new_matcher(ptr: *mut *mut Matcher) -> *const c_char {
    let r = match Matcher::new() {
        Ok(m) => {
            unsafe {
                *ptr = Box::into_raw(Box::new(m));
            }
            Ok(())
        }
        Err(err) => Err(err),
    };
    return_c_error(&r)
}

#[no_mangle]
pub extern "C" fn free_matcher(matcher: *mut Matcher) {
    unsafe { Box::from_raw(matcher) };
    ()
}

#[no_mangle]
pub extern "C" fn update_matcher(matcher: *mut Matcher, path: *const c_char) -> *const c_char {
    let r = unsafe {
        matcher
            .as_mut()
            .ok_or(anyhow!("Invalid matcher"))
            .and_then(|m| Ok(m.update(CStr::from_ptr(path).to_str()?)))
    };
    return_c_error(&r)
}

// #[no_mangle]
// pub extern "C" fn incremental_match<'a, 'b, 'c>(
//     matcher: *const Matcher,
//     query: *const c_char,
//     context: *const c_char,
//     num_matches: u64,
//     lines_ptr: *const RawLine,
//     num_lines: u64,
//     incremental_matcher: *mut *mut IncrementalMatcher<'a, 'b, 'c, RawLine>,
// ) -> *const c_char {
//     let res: Result<()> = try {
//         // TODO: avoid allocating a vector?
//         let lines = unsafe { std::slice::from_raw_parts(lines_ptr, num_lines as usize) };
//         let q = unsafe { CStr::from_ptr(query).to_str()? };
//         let c = unsafe { CStr::from_ptr(context).to_str()? };
//         let inc_matcher = matcher.as_ref().context("invalid pointer")?.incremental_match(q, c, num_matches, lines.as_ref());
//
//         let (stop_send, stop_receive) = channel();
//         let (result_send, result_receive) = channel();
//         thread::spawn(move || {
//             let res = try {
//             let mut progress = inc_matcher.process(100)?;
//             while progress == Progress::Working && !stop_receive.try_recv().is_err() {
//                 progress = inc_matcher.process(100)?;
//             }
//             if let Progress::Done(result) = progress {
//                 result_send.send(result);
//             }
//             };
//         });
//
//         unsafe {
//             *incremental_matcher = Box::into_raw(Box::new(
//                 matcher
//                     .as_ref()
//                     .context("invalid pointer")?
//                     .incremental_match(q, c, num_matches, lines.as_ref()),
//             ))
//         };
//     };
//     return_c_error(&res)
// }

// TODO: XXX: this function is not needed. Should spawn a new thread and run process on that.
// Thread will send an async notify back when it is done.
// #[no_mangle]
// pub extern "C" fn process<'a, 'b, 'c>(
//     inc_matcher: *mut IncrementalMatcher<'a, 'b, 'c, RawLine>,
//     num_lines: usize,
// ) -> *const c_char {
//     let res: Result<()> = try {
//         let progress = unsafe {
//             inc_matcher
//                 .as_mut()
//                 .context("null IncrementalMatcher pointer")?
//                 .process(num_lines)?
//         };
//         match progress {
//             Done(results) => {
//             },
//             Working =>
//         }
//     };
//     return_c_error(&res)
// }

#[no_mangle]
pub extern "C" fn best_matches_c(
    matcher: *const Matcher,
    query: *const c_char,
    context: *const c_char,
    num_matches: u64,
    lines_ptr: *const RawLine,
    num_lines: u64,
    result_ptr: *mut Match,
    num_results: *mut u64,
) -> *const c_char {
    let res: Result<()> = try {
        // TODO: avoid allocating a vector?
        let lines = unsafe { std::slice::from_raw_parts(lines_ptr, num_lines as usize) };
        let q = unsafe { CStr::from_ptr(query).to_str()? };
        let c = unsafe { CStr::from_ptr(context).to_str()? };

        let mtchs = unsafe {
            matcher.as_ref().context("invalid pointer")?.best_matches(
                q,
                c,
                num_matches,
                lines.as_ref(),
            )?
        };
        // copy matches into result vector
        let result = unsafe { std::slice::from_raw_parts_mut(result_ptr, num_matches as usize) };
        unsafe { *num_results = mtchs.len() as u64 };
        for i in 0..mtchs.len() {
            result[i] = mtchs[i].clone()
        }
        ()
    };
    return_c_error(&res)
}
