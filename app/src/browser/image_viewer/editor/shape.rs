use super::*;

pub(super) fn render(shape: Shape, stroke: f64) -> Html {
    match shape.kind {
        Kind::Rectangle => {
            let (low, high) = shape.bounds();
            html! {
                <>
                    <rect class="image-annotation-mark" x={low.x.to_string()} y={low.y.to_string()} width={(high.x-low.x).to_string()} height={(high.y-low.y).to_string()} fill="none" stroke="#ef4444" stroke-width={stroke.to_string()} />
                    <rect class="image-annotation-hit" x={low.x.to_string()} y={low.y.to_string()} width={(high.x-low.x).to_string()} height={(high.y-low.y).to_string()} />
                </>
            }
        }
        Kind::Arrow => {
            let head = shape.arrowhead(stroke);
            html! {
                <>
                    <line class="image-annotation-mark" x1={shape.start.x.to_string()} y1={shape.start.y.to_string()} x2={shape.end.x.to_string()} y2={shape.end.y.to_string()} stroke="#ef4444" stroke-width={stroke.to_string()} stroke-linecap="round" />
                    <polygon fill="#ef4444" points={format!("{},{} {},{} {},{}", head[0].0, head[0].1, head[1].0, head[1].1, head[2].0, head[2].1)} />
                    <line class="image-annotation-hit" x1={shape.start.x.to_string()} y1={shape.start.y.to_string()} x2={shape.end.x.to_string()} y2={shape.end.y.to_string()} />
                </>
            }
        }
    }
}
