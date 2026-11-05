//! Standard textured-color rendering with source-over destination alpha.
//!
//! RGB uses Macroquad 0.4.16's stock shader and blend equation. Alpha accumulates
//! coverage separately, so drawing translucent edges over an opaque scene does
//! not make the completed scene translucent when it is displayed again.
use anyhow::{Context, Result};
use macroquad::{
    material::{load_material, Material, MaterialParams},
    miniquad::{
        Backend, BlendFactor, BlendState, BlendValue, Equation, PipelineParams, ShaderSource,
    },
};

/// Create once while the graphics context is active, then use for scene draws.
pub fn scene_material() -> Result<Material> {
    let backend = unsafe { macroquad::window::get_internal_gl() }
        .quad_context
        .info()
        .backend;
    let shader = match backend {
        Backend::OpenGl => ShaderSource::Glsl {
            vertex: VERTEX,
            fragment: FRAGMENT,
        },
        Backend::Metal => ShaderSource::Msl { program: METAL },
    };
    load_material(
        shader,
        MaterialParams {
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::Value(BlendValue::SourceAlpha),
                    BlendFactor::OneMinusValue(BlendValue::SourceAlpha),
                )),
                alpha_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::One,
                    BlendFactor::OneMinusValue(BlendValue::SourceAlpha),
                )),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .context("create source-over scene material")
}

// Adapted from Macroquad 0.4.16's default textured-color shaders, by Fedor
// Logachev and contributors (MIT; included in packaged dependency notices).
// No color-space, palette or alpha changes occur in the shader; coverage is a
// pipeline rule.
const VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}
"#;

const FRAGMENT: &str = r#"#version 100
varying lowp vec4 color;
varying lowp vec2 uv;

uniform sampler2D Texture;

void main() {
    gl_FragColor = color * texture2D(Texture, uv);
}
"#;

const METAL: &str = r#"
#include <metal_stdlib>
using namespace metal;

// Macroquad's material metadata inserts these uniforms in this exact order.
struct Uniforms {
    float4x4 Projection;
    float4x4 Model;
    float4 _Time;
};

struct Vertex {
    float3 position [[attribute(0)]];
    float2 texcoord [[attribute(1)]];
    float4 color0 [[attribute(2)]];
};

struct RasterizerData {
    float4 position [[position]];
    float4 color [[user(locn0)]];
    float2 uv [[user(locn1)]];
};

vertex RasterizerData vertexShader(Vertex v [[stage_in]], constant Uniforms& uniforms [[buffer(0)]]) {
    RasterizerData out;
    out.position = uniforms.Projection * uniforms.Model * float4(v.position, 1);
    out.color = v.color0 / 255.0;
    out.uv = v.texcoord;
    return out;
}

fragment float4 fragmentShader(RasterizerData in [[stage_in]],
                               texture2d<float> tex [[texture(0)]],
                               sampler texSmplr [[sampler(0)]]) {
    return in.color * tex.sample(texSmplr, in.uv);
}
"#;
