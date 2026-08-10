//! Vendor and generated-Markdown media normalization.

use serde_json::Value;
use wire::Media;

use crate::ResidueKind;

pub(crate) fn markdown(text: &str) -> Result<Vec<Media>, ResidueKind> {
    let media = wire::media::parse_markdown(text)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| malformed())?;
    unique(media)
}

pub(crate) fn value(value: &Value, default_name: &str) -> Result<Vec<Media>, ResidueKind> {
    let mut media = Vec::new();
    collect(value, default_name, &mut media)?;
    unique(media)
}

fn collect(value: &Value, default_name: &str, media: &mut Vec<Media>) -> Result<(), ResidueKind> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect(value, default_name, media)?;
            }
        }
        Value::String(text) => {
            media.extend(markdown(text)?);
            if matches!(text.trim_start().chars().next(), Some('[' | '{'))
                && let Ok(document) = serde_json::from_str::<Value>(text)
            {
                collect(&document, default_name, media)?;
            }
        }
        Value::Object(object) => {
            let name = object
                .get("name")
                .or_else(|| object.get("filename"))
                .and_then(Value::as_str)
                .unwrap_or(default_name);
            if let Some(url) = object
                .get("data_url")
                .or_else(|| object.get("image_url"))
                .or_else(|| object.get("url"))
                .and_then(Value::as_str)
                .filter(|url| url.starts_with("data:image/"))
            {
                media.push(admit(name, url)?);
                return Ok(());
            }
            if object.get("type").and_then(Value::as_str) == Some("image") {
                if let Some(source) = object.get("source").and_then(Value::as_object)
                    && source.get("type").and_then(Value::as_str) == Some("base64")
                {
                    let content_type = source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .ok_or_else(malformed)?;
                    let data = source
                        .get("data")
                        .and_then(Value::as_str)
                        .ok_or_else(malformed)?;
                    media.push(
                        Media::from_base64(name, content_type, data).map_err(|_| malformed())?,
                    );
                    return Ok(());
                }
                if let Some(data) = object.get("data").and_then(Value::as_str) {
                    let content_type = object
                        .get("mimeType")
                        .or_else(|| object.get("mime_type"))
                        .or_else(|| object.get("media_type"))
                        .and_then(Value::as_str)
                        .ok_or_else(malformed)?;
                    media.push(
                        Media::from_base64(name, content_type, data).map_err(|_| malformed())?,
                    );
                    return Ok(());
                }
            }
            for field in ["content", "output", "result", "Ok", "Err"] {
                if let Some(value) = object.get(field) {
                    collect(value, default_name, media)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn admit(name: &str, data_url: &str) -> Result<Media, ResidueKind> {
    let content_type = data_url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,").map(|(kind, _)| kind))
        .ok_or_else(malformed)?;
    Media::admit(name, content_type, data_url).map_err(|_| malformed())
}

fn unique(media: Vec<Media>) -> Result<Vec<Media>, ResidueKind> {
    wire::media::admit_media(&media).map_err(|_| malformed())?;
    let mut unique = Vec::<Media>::new();
    for candidate in media {
        let identity = candidate.identity().map_err(|_| malformed())?;
        let duplicate = unique.iter().any(|existing| {
            existing
                .identity()
                .is_ok_and(|existing| existing == identity)
        });
        if !duplicate {
            unique.push(candidate);
        }
    }
    Ok(unique)
}

fn malformed() -> ResidueKind {
    ResidueKind::Malformed {
        field: "media",
        expected: "at most eight canonical PNG, JPEG, GIF, or WebP data blocks",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &str = "iVBORw0KGgo=";

    #[test]
    fn markdown_and_content_block_share_one_identity() {
        let url = format!("data:image/png;base64,{PNG}");
        let document = serde_json::json!([
            {"type": "text", "text": format!("![capture.png]({url})")},
            {"type": "image", "data": PNG, "mimeType": "image/png", "name": "echo.png"},
        ]);
        let media = value(&document, "tool-image.png").unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(
            media[0].identity().unwrap(),
            admit("capture.png", &url).unwrap().identity().unwrap()
        );
    }
}
