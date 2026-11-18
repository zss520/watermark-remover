use crate::template::{LoadedTemplate, Template};

#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: u32,
    pub height: u32,
}

impl Placement {
    pub fn frac(self) -> (f32, f32) {
        let fx = self.origin_x - self.origin_x.floor();
        let fy = self.origin_y - self.origin_y.floor();
        (fx, fy)
    }

    pub fn shifted(self, dx: i32, dy: i32) -> Self {
        Self {
            origin_x: self.origin_x + dx as f32,
            origin_y: self.origin_y + dy as f32,
            ..self
        }
    }

    pub fn fully_inside(self, image_w: u32, image_h: u32) -> bool {
        let x1 = self.origin_x + self.width as f32;
        let y1 = self.origin_y + self.height as f32;
        self.origin_x >= -1e-3
            && self.origin_y >= -1e-3
            && x1 <= image_w as f32 + 1e-3
            && y1 <= image_h as f32 + 1e-3
    }
}

pub fn place(template: &Template, image_w: u32, image_h: u32) -> Option<Placement> {
    if image_w < template.min_image_size[0] || image_h < template.min_image_size[1] {
        return None;
    }
    let m = image_w.min(image_h) as f32;
    let spec = &template.placement;
    if m <= 0.0 || spec.aspect <= 0.0 || spec.width_ratio <= 0.0 {
        return None;
    }
    let width = (spec.width_ratio * m).round();
    if width < spec.min_width_px as f32 || width < 1.0 {
        return None;
    }
    let height = (width / spec.aspect).round();
    if height < 1.0 {
        return None;
    }
    let right = (spec.margin_x * m).round();
    let bottom = (spec.margin_y * m).round();
    let origin_x = image_w as f32 - right - width + spec.subpixel[0];
    let origin_y = image_h as f32 - bottom - height + spec.subpixel[1];
    Some(Placement {
        origin_x,
        origin_y,
        width: width as u32,
        height: height as u32,
    })
}

pub fn place_loaded(template: &LoadedTemplate, image_w: u32, image_h: u32) -> Option<Placement> {
    place(&template.meta, image_w, image_h)
}
