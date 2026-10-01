use super::*;
use crate::image_annotation::{Kind, stroke_width};

pub(super) async fn render(
    original: &wire::Media,
    shapes: &[Shape],
) -> Result<wire::Media, JsValue> {
    if shapes.is_empty() {
        return Ok(original.clone());
    }
    let image = web_sys::HtmlImageElement::new()?;
    image.set_src(&original.data_url);
    JsFuture::from(image.decode()).await?;
    let canvas = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("No document"))?
        .create_element("canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    canvas.set_width(image.natural_width());
    canvas.set_height(image.natural_height());
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("Image export unavailable"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()?;
    context.draw_image_with_html_image_element(&image, 0.0, 0.0)?;
    let stroke = stroke_width(image.natural_width(), image.natural_height());
    context.set_stroke_style_str("#ef4444");
    context.set_fill_style_str("#ef4444");
    context.set_line_width(stroke);
    context.set_line_join("round");
    context.set_line_cap("round");
    for shape in shapes {
        match shape.kind {
            Kind::Rectangle => {
                let (low, high) = shape.bounds();
                context.stroke_rect(
                    f64::from(low.x),
                    f64::from(low.y),
                    f64::from(high.x - low.x),
                    f64::from(high.y - low.y),
                );
            }
            Kind::Arrow => {
                context.begin_path();
                context.move_to(f64::from(shape.start.x), f64::from(shape.start.y));
                context.line_to(f64::from(shape.end.x), f64::from(shape.end.y));
                context.stroke();
                let [tip, left, right] = shape.arrowhead(stroke);
                context.begin_path();
                context.move_to(tip.0, tip.1);
                context.line_to(left.0, left.1);
                context.line_to(right.0, right.1);
                context.close_path();
                context.fill();
            }
        }
    }
    let data_url = canvas.to_data_url_with_type("image/png")?;
    let stem = original
        .name
        .rsplit_once('.')
        .map_or(original.name.as_str(), |(stem, _)| stem);
    wire::Media::admit(format!("{stem}.png"), "image/png", data_url)
        .map_err(|error| JsValue::from_str(&error.to_string()))
}
