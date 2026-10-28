//! Original desktop cover lookup order, independent of image/GPU services.
use std::path::{Path,PathBuf};
pub fn cover_path_candidates(note:&Path,value:&str,covers:&Path)->Vec<PathBuf>{
 let raw=Path::new(value);
 if raw.is_absolute(){return vec![raw.to_path_buf()];}
 if value.contains('/'){return note.parent().map(|parent|vec![parent.join(raw)]).unwrap_or_default();}
 let mut candidates=vec![covers.join(value)];
 candidates.extend(["png","jpg","jpeg","webp"].map(|extension|covers.join(format!("{value}.{extension}"))));candidates
}
