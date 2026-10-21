use serde::{Deserialize, Serialize};

use crate::types::{Base64ImageSource, CacheControlEphemeral, FileSource, UrlImageSource};

/// The source type for an image block, which can be either Base64 encoded or a URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ImageSource {
    /// A Base64 encoded image source.
    #[serde(rename = "base64")]
    Base64(Base64ImageSource),

    /// A URL image source.
    #[serde(rename = "url")]
    Url(UrlImageSource),

    /// A file uploaded through the Files API.
    #[serde(rename = "file")]
    File(FileSource),
}

/// What to do with an image larger than the supported size.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OversizedImage {
    /// Downsize the image to fit.
    Downsize,
    /// Reject the request.
    Error,
}

/// Transformations applied to an image before the model sees it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImageTransformations {
    /// How oversized images are handled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oversized_image: Option<OversizedImage>,
}

impl ImageTransformations {
    /// Set how an image larger than the supported size is handled.
    #[must_use]
    pub fn with_oversized_image(mut self, oversized_image: OversizedImage) -> Self {
        self.oversized_image = Some(oversized_image);
        self
    }
}

/// Parameters for an image block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ImageBlock {
    /// The source of the image.
    pub source: ImageSource,

    /// Create a cache control breakpoint at this content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// Transformations applied to the image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transformations: Option<ImageTransformations>,
}

impl ImageBlock {
    /// Create a new `ImageBlock` with the given source.
    #[must_use]
    pub fn new(source: ImageSource) -> Self {
        Self {
            source,
            cache_control: None,
            transformations: None,
        }
    }

    /// Set the image transformations.
    #[must_use]
    pub fn with_transformations(mut self, transformations: ImageTransformations) -> Self {
        self.transformations = Some(transformations);
        self
    }

    /// Add a cache control to this image block.
    #[must_use]
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::base64_image_source::ImageMediaType;
    use serde_json::{json, to_value};

    #[test]
    fn image_block_with_base64() {
        let base64_source = Base64ImageSource::new(
            "data:image/jpeg;base64,SGVsbG8gd29ybGQ=",
            ImageMediaType::Jpeg,
        );

        let image_block = ImageBlock::new(ImageSource::Base64(base64_source));
        let json = to_value(&image_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "base64",
                    "data": "data:image/jpeg;base64,SGVsbG8gd29ybGQ=",
                    "media_type": "image/jpeg"
                }
            })
        );
    }

    #[test]
    fn image_block_with_url() {
        let url_source = UrlImageSource::new("https://example.com/image.jpg");

        let image_block = ImageBlock::new(ImageSource::Url(url_source));
        let json = to_value(&image_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "url",
                    "url": "https://example.com/image.jpg"
                }
            })
        );
    }

    #[test]
    fn image_block_with_cache_control() {
        let url_source = UrlImageSource::new("https://example.com/image.jpg");
        let cache_control = CacheControlEphemeral::new();

        let image_block =
            ImageBlock::new(ImageSource::Url(url_source)).with_cache_control(cache_control);

        let json = to_value(&image_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "url",
                    "url": "https://example.com/image.jpg"
                },
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn image_block_with_file_source_and_transformations() {
        let block = ImageBlock::new(ImageSource::File(FileSource::new("file_02")))
            .with_transformations(ImageTransformations {
                oversized_image: Some(OversizedImage::Downsize),
            });
        let json = to_value(&block).unwrap();
        assert_eq!(
            json,
            json!({
                "source": {"type": "file", "file_id": "file_02"},
                "transformations": {"oversized_image": "downsize"}
            })
        );
        let parsed: ImageBlock = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, block);
    }
}
