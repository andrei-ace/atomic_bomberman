//! Development-only JSON-lines bridge for tools/compare_ai.py.
use std::io::{self, BufRead};
fn main() {
    for line in io::stdin().lock().lines() {
        let result = line
            .map_err(|e| e.to_string())
            .and_then(|line| serde_json::from_str(&line).map_err(|e| e.to_string()))
            .and_then(bomberman_core::original_ai::evaluate);
        match result {
            Ok(value) => println!("{}", serde_json::to_string(&value).unwrap()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
}
