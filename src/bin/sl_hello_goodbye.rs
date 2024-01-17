#![deny(unknown_lints)]
#![deny(renamed_and_removed_lints)]
#![forbid(unsafe_code)]
#![deny(deprecated)]
#![forbid(private_interfaces)]
#![forbid(private_bounds)]
#![forbid(non_fmt_panics)]
#![deny(unreachable_code)]
#![deny(unreachable_patterns)]
#![forbid(unused_doc_comments)]
#![forbid(unused_must_use)]
#![deny(while_true)]
#![deny(unused_parens)]
#![deny(redundant_semicolons)]
#![deny(non_ascii_idents)]
#![deny(confusable_idents)]
#![warn(missing_docs)]
#![warn(clippy::missing_docs_in_private_items)]
#![warn(clippy::cargo_common_metadata)]
#![warn(rustdoc::missing_crate_level_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(missing_debug_implementations)]
#![deny(clippy::mod_module_files)]
//#![warn(clippy::pedantic)]
#![warn(clippy::redundant_else)]
#![warn(clippy::must_use_candidate)]
#![warn(clippy::missing_panics_doc)]
#![warn(clippy::missing_errors_doc)]
#![warn(clippy::panic)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![doc = include_str!("../../README.md")]

use clap::Parser;

use tracing::instrument;
use tracing_subscriber::{
    filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer, Registry,
};

/// Error enum for the application
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// error reading environment variable
    #[error("error when retrieving environment variable: {0}")]
    EnvVarError(#[from] std::env::VarError),
    /// error in clap
    #[error("error in CLI option parsing: {0}")]
    ClapError(#[from] clap::Error),
    /// error parsing log filter
    #[error("error parsing log filter: {0}")]
    LogFilterParseError(#[from] tracing_subscriber::filter::ParseError),
}

/// The Clap type for all the commandline parameters
#[derive(clap::Parser, Debug)]
#[clap(name = clap::crate_name!(),
       about = clap::crate_description!(),
       author = clap::crate_authors!(),
       version = clap::crate_version!(),
       )]
struct Options {}

/// The main behaviour of the binary should go here
#[instrument]
async fn do_stuff() -> Result<(), crate::Error> {
    let options = Options::parse();
    tracing::debug!("{:#?}", options);

    // main code goes here

    Ok(())
}

/// The main function mainly just handles setting up tracing
/// and handling any Err Results.
#[tokio::main]
async fn main() -> Result<(), Error> {
    let terminal_env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .parse(std::env::var("RUST_LOG").unwrap_or_else(|_| "".to_string()))?;
    let file_env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::TRACE.into())
        .parse(std::env::var("SL_HELLO_GOODBYE_LOG").unwrap_or_else(|_| "".to_string()))?;
    let registry = Registry::default();
    let registry =
        registry.with(tracing_subscriber::fmt::Layer::default().with_filter(terminal_env_filter));
    let log_dir = std::env::var("SL_HELLO_GOODBYE_LOG_DIR");
    if let Ok(log_dir) = log_dir {
        let log_file = if let Ok(log_file) = std::env::var("SL_HELLO_GOODBYE_LOG_FILE") {
            log_file
        } else {
            "sl_hello_goodbye.log".to_string()
        };
        tracing::info!("Logging to {}/{}", log_dir, log_file);
        let file_appender = tracing_appender::rolling::never(log_dir, log_file);
        registry
            .with(
                tracing_subscriber::fmt::Layer::default()
                    .with_writer(file_appender)
                    .with_filter(file_env_filter),
            )
            .init();
    } else {
        registry.init();
    }
    log_panics::init();
    match do_stuff().await {
        Ok(_) => (),
        Err(e) => {
            tracing::error!("{}", e);
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
    tracing::debug!("Exiting");
    Ok(())
}

#[cfg(test)]
mod test {
    //use super::*;
    //use pretty_assertions::{assert_eq, assert_ne};
}
