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
    animation: Option<AnimationAsset>,
    sequence: usize,
    frame: u32,
    anchor: [i32; 2],
    #[serde(default)]
    floor_flame: bool,
    #[serde(default)]
    ground_sprite: bool,
}
fn main() {
    let mut animation = None;
    for line in io::stdin().lock().lines() {
        let f: Fixture = serde_json::from_str(&line.expect("fixture line")).expect("parse fixture");
        if let Some(asset) = f.animation {
            animation = Some(asset);
        }
        let animation = animation.as_ref().expect("first fixture needs animation");
        let sequence = &animation.sequences[f.sequence];
        let image = presentation::native_image(sequence, f.frame as usize).expect("native image");
        let frame = &animation.frames[image as usize];
        let anchor = (f.anchor[0] as f32, f.anchor[1] as f32);
        let anchor = if f.floor_flame {
            presentation::flame_anchor(
                anchor.0,
                anchor.1,
                presentation::native_metadata(sequence, f.frame as usize),
            )
        } else if f.ground_sprite {
            presentation::ground_anchor(anchor.0, anchor.1)
        } else {
            anchor
        };
        let (x, y) = presentation::image_position(frame, anchor.0, anchor.1);
        println!(
            "{}",
            json!({"image":image,"metadata":presentation::native_metadata(sequence,f.frame as usize),
            "top_left":[x as i32,y as i32],"size":[frame.width,frame.height]})
        );
    }
}
