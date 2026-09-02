//! Typed vendor-content decomposition and generated-Markdown media normalization.

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

pub(crate) struct Content {
    pub(crate) text: String,
    pub(crate) media: Vec<Media>,
}

/// Decompose one provider-owned content value without interpreting strings.
///
/// Structured image blocks are admitted before text fields are considered.
/// String values remain exact output text: they are never reparsed as JSON or
/// Markdown, even when their bytes happen to spell an image block.
pub(crate) fn content(value: &Value, default_name: &str) -> Result<Content, ResidueKind> {
    let mut text = Vec::new();
    let mut media = Vec::new();
    collect(value, default_name, &mut text, &mut media)?;
    Ok(Content {
        text: text.join("\n"),
        media: unique(media)?,
    })
}

fn collect(
    value: &Value,
    default_name: &str,
    text: &mut Vec<String>,
    media: &mut Vec<Media>,
) -> Result<(), ResidueKind> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect(value, default_name, text, media)?;
            }
        }
        Value::String(value) => {
            text.push(value.clone());
        }
        Value::Object(object) => {
            if let Some(candidate) = image(object, default_name)? {
                media.push(candidate);
                return Ok(());
            }
            if let Some(value) = object
                .get("text")
                .or_else(|| object.get("content").filter(|value| value.is_string()))
                .and_then(Value::as_str)
            {
                text.push(value.to_owned());
                return Ok(());
            }
            for field in ["content", "output", "result", "Ok", "Err"] {
                if let Some(value) = object.get(field) {
                    collect(value, default_name, text, media)?;
                }
            }
        }
        Value::Null => {}
        Value::Bool(value) => text.push(value.to_string()),
        Value::Number(value) => text.push(value.to_string()),
    }
    Ok(())
}

fn image(
    object: &serde_json::Map<String, Value>,
    default_name: &str,
) -> Result<Option<Media>, ResidueKind> {
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    if !matches!(kind, "image" | "input_image" | "output_image") {
        return Ok(None);
    }

    let name = object
        .get("name")
        .or_else(|| object.get("filename"))
        .and_then(Value::as_str)
        .unwrap_or(default_name);
    if let Some(url) = object
        .get("data_url")
        .or_else(|| object.get("image_url"))
        .or_else(|| object.get("url"))
        .map(|value| value.as_str().ok_or_else(malformed))
        .transpose()?
    {
        return admit(name, url).map(Some);
    }
    if let Some(source) = object.get("source") {
        let source = source.as_object().ok_or_else(malformed)?;
        if source.get("type").and_then(Value::as_str) != Some("base64") {
            return Err(malformed());
        }
        let content_type = source
            .get("media_type")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        let data = source
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        return Media::from_base64(name, content_type, data)
            .map(Some)
            .map_err(|_| malformed());
    }
    if let Some(data) = object.get("data") {
        let data = data.as_str().ok_or_else(malformed)?;
        let content_type = object
            .get("mimeType")
            .or_else(|| object.get("mime_type"))
            .or_else(|| object.get("media_type"))
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        return Media::from_base64(name, content_type, data)
            .map(Some)
            .map_err(|_| malformed());
    }
    Err(malformed())
}

fn admit(name: &str, data_url: &str) -> Result<Media, ResidueKind> {
    let content_type = data_url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,").map(|(kind, _)| kind))
        .ok_or_else(malformed)?;
    Media::admit(name, content_type, data_url).map_err(|_| malformed())
}

pub(crate) fn unique(media: Vec<Media>) -> Result<Vec<Media>, ResidueKind> {
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
        let content = content(&document, "tool-image.png").unwrap();
        assert_eq!(content.text, format!("![capture.png]({url})"));
        assert_eq!(content.media.len(), 1);
        assert_eq!(
            content.media[0].identity().unwrap(),
            admit("capture.png", &url).unwrap().identity().unwrap()
        );
    }

    #[test]
    fn strings_are_opaque_even_when_they_spell_media() {
        let url = format!("data:image/png;base64,{PNG}");
        let text = format!("![capture.png]({url})\n{{\"type\":\"image\",\"image_url\":\"{url}\"}}");
        let content = content(&Value::String(text.clone()), "tool-image.png").unwrap();
        assert_eq!(content.text, text);
        assert!(content.media.is_empty());
    }

    #[test]
    fn malformed_typed_image_is_a_media_error() {
        let block = serde_json::json!({"type": "image", "data": PNG});
        assert!(matches!(
            content(&block, "tool-image.png"),
            Err(ResidueKind::Malformed { field: "media", .. })
        ));
    }
}
