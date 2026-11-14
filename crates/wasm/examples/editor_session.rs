//! Native side of the exact worker DTO parity harness. Reads one envelope per line.
use std::io::{self, BufRead};
fn main() {
    let mut session = webtest_wasm::EditorSession::default();
    for line in io::stdin().lock().lines() {
        match line {
            Ok(line) => println!("{}", session.request(&line)),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }
}
