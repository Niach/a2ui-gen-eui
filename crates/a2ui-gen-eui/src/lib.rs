//! Generates A2UI Express user interfaces with a small local model.
//!
//! - [`Catalog`]: a catalog schema, the [grammar](grammar) and system prompt made from it, and the
//!   `a2ui-express` compiler that validates replies against it. [`Catalog::eui`] is built in.
//! - [`Generator`] (feature `llama`): a GGUF model in llama.cpp. [`Generator::generate`] and
//!   [`Generator::refine`] return a [`Generation`], a stream of [`ExpressBlock`]s.
//! - [`model`]: sha256 pinning and, with the feature `download`, an explicit download.
//!
//! ```no_run
//! # #[cfg(feature = "llama")] fn main() -> Result<(), a2ui_gen_eui::Error> {
//! use a2ui_gen_eui::{ExpressBlock, Generator};
//!
//! let generator = Generator::load("model.gguf")?;
//! for block in generator.generate("A settings screen for notifications", None, None)?.blocking() {
//!     match block? {
//!         ExpressBlock::Statement { source, .. } => println!("{source}"),
//!         ExpressBlock::Messages(_messages) => { /* A2UI JSON, hand it to the renderer */ }
//!         ExpressBlock::Restart { .. } => { /* drop the surface, a new attempt starts */ }
//!         ExpressBlock::Complete(ui) => println!("valid after {} attempt(s)", ui.attempts),
//!     }
//! }
//! # Ok(()) }
//! # #[cfg(not(feature = "llama"))] fn main() {}
//! ```

mod catalog;
pub mod constraint;
#[cfg(feature = "llama")]
mod engine;
mod error;
pub mod grammar;
pub mod model;
pub mod prompt;
#[cfg(any(feature = "llama", test))]
mod sample;
pub mod stream;

pub use catalog::{Catalog, Validated};
#[cfg(feature = "llama")]
pub use engine::{Decoding, ExpressBlock, Generation, Generator, LoadOptions, Options, Stats, Ui};
pub use error::{Error, Result};
pub use stream::StatementKind;

/// The id of the surface every reply builds.
pub const SURFACE_ID: &str = "main";
