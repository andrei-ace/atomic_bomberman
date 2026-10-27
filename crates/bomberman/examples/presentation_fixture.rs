//! Development bridge for original first-image lookup and CIMG placement.
#[allow(dead_code)]
#[path = "../src/presentation.rs"]
mod presentation;
use bomberman_assets::AnimationAsset;
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
#[derive(Deserialize)]
struct Fixture {
    animation: AnimationAsset,
    sequence: usize,
    frame: u32,
    anchor: [i32; 2],
}
fn main() {
    for line in io::stdin().lock().lines() {
        let f: Fixture = serde_json::from_str(&line.expect("fixture line")).expect("parse fixture");
        let sequence = &f.animation.sequences[f.sequence];
        let image = presentation::native_image(sequence, f.frame as usize).expect("native image");
        let frame = &f.animation.frames[image as usize];
        let (x, y) = presentation::image_position(frame, f.anchor[0] as f32, f.anchor[1] as f32);
        println!(
            "{}",
            json!({"image":image,"metadata":presentation::native_metadata(sequence,f.frame as usize),
            "top_left":[x as i32,y as i32],"size":[frame.width,frame.height]})
        );
    }
}
