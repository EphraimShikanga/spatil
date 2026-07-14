use crate::config::{Aether, Display, Strata, parse_hex_color};
use crate::scene::WORLD_SCALE;
use crate::scene::mode::CameraFraming;

pub struct StrataParams {
    pub grid_color: [f32; 4],
    pub void_color: [f32; 4],
    pub axis_color: [f32; 4],
    pub floor_half: [f32; 2],
    pub light_position: [f32; 3],
    pub grid_spacing: f32,
    pub grid_extent: f32,
    pub grid_level: f32,
    pub fog_density: f32,
    pub camera_distance: f32,
    pub camera_elevation: f32,
    pub camera_orbit: f32,
    pub enable_shadows: bool,
}

impl StrataParams {
    pub fn from_config(strata: &Strata, display: &Display) -> Self {
        Self {
            grid_color: parse_hex_color(&strata.grid_color),
            void_color: parse_hex_color(&strata.void_color),
            axis_color: parse_hex_color(&strata.axis_color),
            floor_half: floor_half_extent(display),
            light_position: strata.light_position,
            grid_spacing: strata.grid_spacing,
            grid_extent: strata.grid_extent,
            grid_level: strata.grid_level,
            fog_density: strata.fog_density,
            camera_distance: strata.camera_distance,
            camera_elevation: strata.camera_elevation,
            camera_orbit: strata.camera_orbit,
            enable_shadows: strata.enable_shadows,
        }
    }

    pub fn camera_framing(&self) -> CameraFraming {
        CameraFraming {
            distance: self.camera_distance,
            elevation: self.camera_elevation,
            orbit: self.camera_orbit,
        }
    }
}

pub struct AetherParams {
    pub void_color: [f32; 4],
    pub particle_color: [f32; 4],
    pub glass_tint: [f32; 4],
    pub glass_alpha: f32,
    pub particle_size: f32,
    pub fog_density: f32,
    pub camera_distance: f32,
    pub field: [f32; 3],
    pub particle_count: u32,
    pub parallax_enabled: bool,
    pub parallax_strength: f32,
}

impl AetherParams {
    pub fn from_config(aether: &Aether) -> Self {
        Self {
            void_color: parse_hex_color(&aether.void_color),
            particle_color: particle_color(aether),
            glass_tint: glass_tint(aether),
            glass_alpha: aether.glass_alpha,
            particle_size: aether.particle_size,
            fog_density: aether.fog_density,
            camera_distance: aether.camera_distance,
            field: [aether.field_width, aether.field_height, aether.field_depth],
            particle_count: aether.particle_count,
            parallax_enabled: aether.parallax_enabled,
            parallax_strength: aether.parallax_strength,
        }
    }

    pub fn parallax(&self) -> f32 {
        if self.parallax_enabled {
            self.parallax_strength
        } else {
            0.0
        }
    }
}

fn glass_tint(aether: &Aether) -> [f32; 4] {
    let rgb = parse_hex_color(&aether.glass_tint);
    [rgb[0], rgb[1], rgb[2], aether.glass_tint_strength]
}

fn particle_color(aether: &Aether) -> [f32; 4] {
    let c = parse_hex_color(&aether.particle_color);
    [c[0], c[1], c[2], aether.particle_opacity]
}

fn floor_half_extent(display: &Display) -> [f32; 2] {
    [
        display.width as f32 / WORLD_SCALE / 2.0,
        display.height as f32 / WORLD_SCALE / 2.0,
    ]
}
