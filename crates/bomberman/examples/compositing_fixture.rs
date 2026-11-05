//! Native GPU regression for translucent rendering followed by window presentation.
#[path = "../src/presentation/compositing.rs"]
mod compositing;

use macroquad::{miniquad::conf::AppleGfxApi, prelude::*};

fn window_conf() -> Conf {
    let metal = std::env::args().any(|arg| arg == "--metal");
    Conf {
        window_title: "Atomic Bomberman compositing check".into(),
        window_width: 128,
        window_height: 96,
        platform: miniquad::conf::Platform {
            apple_gfx_api: if metal {
                AppleGfxApi::Metal
            } else {
                AppleGfxApi::OpenGl
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let material = compositing::scene_material().expect("source-over scene material");
    let target = render_target(64, 48);
    target.texture.set_filter(FilterMode::Nearest);
    let mut camera = Camera2D::from_display_rect(Rect::new(0., 0., 64., 48.));
    camera.render_target = Some(target.clone());
    set_camera(&camera);
    gl_use_material(&material);
    clear_background(Color::new(0., 0., 1., 1.));
    draw_rectangle(0., 0., 64., 48., Color::new(1., 0., 0., 0.5));
    draw_rectangle(0., 0., 64., 48., Color::new(0., 1., 0., 0.25));
    set_default_camera();
    gl_use_default_material();
    clear_background(BLACK);
    draw_texture_ex(
        &target.texture,
        0.,
        0.,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(screen_width(), screen_height())),
            flip_y: true,
            ..Default::default()
        },
    );
    let backend = unsafe { macroquad::window::get_internal_gl() }
        .quad_context
        .info()
        .backend;
    if backend == miniquad::Backend::Metal {
        // Miniquad's Metal backend has no texture_read_pixels implementation.
        // Still compile both shaders and submit/present the real scene.
        next_frame().await;
        println!("Metal shaders compiled and scene presented; framebuffer readback is unavailable");
        return;
    }
    let canvas = target.texture.get_texture_data();
    let window = get_screen_data();
    let sample = |image: &Image| {
        let offset = (usize::from(image.height / 2) * usize::from(image.width)
            + usize::from(image.width / 2))
            * 4;
        image.bytes[offset..offset + 4].to_vec()
    };
    let canvas_pixel = sample(&canvas);
    let window_pixel = sample(&window);
    assert_eq!(canvas_pixel[3], 255, "overlaid opaque canvas lost opacity");
    for (actual, expected) in canvas_pixel[..3].iter().zip([96_u8, 64, 96]) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "source-over RGB: {canvas_pixel:?}"
        );
    }
    assert_eq!(
        &canvas_pixel[..3],
        &window_pixel[..3],
        "window darkened the rendered canvas"
    );
    println!("Compositing passed: {backend:?}, canvas={canvas_pixel:?}, window={window_pixel:?}");
    next_frame().await;
}
