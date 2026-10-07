// use crate::{Hasher, Rectangle, Size};
use crate::components::core::shapes::Hasher;

use std::hash::{Hash, Hasher as _};
use std::path::PathBuf;
use std::sync::Arc;

/// A handle of some image data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handle {
    id: u64,
    pub data: Data,
}

impl Handle {
    /// Creates an image [`Handle`] pointing to the image of the given path.
    ///
    /// Makes an educated guess about the image format by examining the data in the file.
    pub fn from_path<T: Into<PathBuf>>(path: T) -> Handle {
        Self::from_data(Data::Path(path.into()))
    }

    /// Creates an image [`Handle`] containing the image pixels directly. This
    /// function expects the input data to be provided as a `Vec<u8>` of RGBA
    /// pixels.
    ///
    /// This is useful if you have already decoded your image.
    pub fn from_pixels(
        width: u32,
        height: u32,
        pixels: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> Handle {
        Self::from_data(Data::Rgba {
            width,
            height,
            pixels: Bytes::new(pixels),
        })
    }

    /// Streaming RGBA identity, without reading or hashing the pixel buffer.
    /// The owner must supply a process-unique stream identity and change the
    /// generation whenever pixels change. Static assets use `from_pixels`.
    pub fn from_stream_pixels(
        width: u32,
        height: u32,
        stream: (u64, u64),
        generation: (u64, u64),
        pixels: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> Handle {
        let mut hasher = Hasher::default();
        ("neoism-stream-rgba", stream, generation, width, height).hash(&mut hasher);
        Handle {
            id: hasher.finish(),
            data: Data::Rgba {
                width,
                height,
                pixels: Bytes::new(pixels),
            },
        }
    }

    /// Creates an image [`Handle`] containing the image data directly.
    ///
    /// Makes an educated guess about the image format by examining the given data.
    ///
    /// This is useful if you already have your image loaded in-memory, maybe
    /// because you downloaded or generated it procedurally.
    pub fn from_memory(bytes: impl AsRef<[u8]> + Send + Sync + 'static) -> Handle {
        Self::from_data(Data::Bytes(Bytes::new(bytes)))
    }

    fn from_data(data: Data) -> Handle {
        let mut hasher = Hasher::default();
        data.hash(&mut hasher);

        Handle {
            id: hasher.finish(),
            data,
        }
    }

    /// Returns the unique identifier of the [`Handle`].
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns a reference to the image [`Data`].
    pub fn data(&self) -> &Data {
        &self.data
    }
}

impl<T> From<T> for Handle
where
    T: Into<PathBuf>,
{
    fn from(path: T) -> Handle {
        Handle::from_path(path.into())
    }
}

impl Hash for Handle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// A wrapper around raw image data.
///
/// It behaves like a `&[u8]`.
#[derive(Clone)]
pub struct Bytes(Arc<dyn AsRef<[u8]> + Send + Sync + 'static>);

impl Bytes {
    /// Creates new [`Bytes`] around `data`.
    pub fn new(data: impl AsRef<[u8]> + Send + Sync + 'static) -> Self {
        Self(Arc::new(data))
    }
}

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.as_ref().as_ref().fmt(f)
    }
}

impl std::hash::Hash for Bytes {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_ref().as_ref().hash(state);
    }
}

impl PartialEq for Bytes {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Eq for Bytes {}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().as_ref()
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.0.as_ref().as_ref()
    }
}

/// The data of a raster image.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Data {
    /// File data
    Path(PathBuf),

    /// In-memory data
    Bytes(Bytes),

    /// Decoded image pixels in RGBA format.
    Rgba {
        /// The width of the image.
        width: u32,
        /// The height of the image.
        height: u32,
        /// The pixels.
        pixels: Bytes,
    },
}

impl std::fmt::Debug for Data {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Data::Path(path) => write!(f, "Path({path:?})"),
            Data::Bytes(_) => write!(f, "Bytes(...)"),
            Data::Rgba { width, height, .. } => {
                write!(f, "Pixels({width} * {height})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_identity_tracks_owner_generation_and_dimensions_not_pixels() {
        let make = |stream, generation, w, h, px: Vec<u8>| {
            Handle::from_stream_pixels(w, h, stream, generation, px).id()
        };
        let id = make((1, 2), (3, 4), 1, 1, vec![0; 4]);
        assert_eq!(id, make((1, 2), (3, 4), 1, 1, vec![255; 4]));
        for other in [
            make((2, 2), (3, 4), 1, 1, vec![0; 4]),
            make((1, 3), (3, 4), 1, 1, vec![0; 4]),
            make((1, 2), (4, 4), 1, 1, vec![0; 4]),
            make((1, 2), (3, 5), 1, 1, vec![0; 4]),
            make((1, 2), (3, 4), 2, 1, vec![0; 8]),
            make((1, 2), (3, 4), 1, 2, vec![0; 8]),
        ] {
            assert_ne!(id, other);
        }
        assert_ne!(
            Handle::from_pixels(1, 1, vec![0; 4]).id(),
            Handle::from_pixels(1, 1, vec![255; 4]).id()
        );
    }

    #[test]
    fn stream_constructor_never_reads_pixel_storage() {
        struct Unreadable;
        impl AsRef<[u8]> for Unreadable {
            fn as_ref(&self) -> &[u8] {
                panic!("constructor read pixels")
            }
        }
        let _ = Handle::from_stream_pixels(1, 1, (1, 2), (3, 4), Unreadable);
    }
}
