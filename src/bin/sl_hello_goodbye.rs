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

use std::collections::BTreeMap;
use std::path::PathBuf;

use chumsky::text::whitespace;
use redb::{ReadableDatabase as _, ReadableTable as _};
use tracing::instrument;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

use ariadne::{Color, Fmt, Label, Report, ReportKind, Source};
use chumsky::{Parser, prelude::*};

/// describes the redb table to store the last seen time
/// the key string is the avatar legacy name, the other one is
/// the formatted time
const LAST_SEEN_TABLE: redb::TableDefinition<String, String> =
    redb::TableDefinition::new("last_seen");

/// format for the timestamps used in the last_seen.db
const TIME_FORMAT: &[time::format_description::BorrowedFormatItem<'_>] =
    time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

/// Error enum for the application
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// error reading environment variable
    #[error("error when retrieving environment variable: {0}")]
    EnvVarError(#[from] std::env::VarError),
    /// error in clap
    #[error("error in CLI option parsing: {0}")]
    ClapError(#[from] clap::Error),
    /// Could not determine database dir
    #[error("Could not determine directory for database storage")]
    CouldNotDetermineDatabaseStorageDir,
    /// error parsing log filter
    #[error("error parsing log filter: {0}")]
    LogFilterParseError(#[from] tracing_subscriber::filter::ParseError),
    /// error determining current user home directory
    #[error("error determining current user home directory")]
    HomeDirError,
    /// local chat file not found
    #[error("local chat file not found: {0}")]
    LocalChatFileNotFound(std::path::PathBuf),
    /// error created MuxedLines
    #[error("error creating MuxedLines: {0}")]
    MuxedLinesError(std::io::Error),
    /// error adding file to MuxedLines
    #[error("error adding file to MuxedLines: {0}")]
    MuxedLinesAddFileError(std::io::Error),
    /// error parsing chat log line
    #[error("error parsing chat log line: {0}")]
    ChatLogLineParseError(ChumskyError),
    /// error joining the log reader task before shutdown
    #[error("error joining the log reader task before shutdown: {0}")]
    JoinError(#[from] tokio::task::JoinError),
    /// redb database error
    #[error("redb database error: {0}")]
    DatabaseError(#[from] redb::DatabaseError),
    /// redb transaction error
    #[error("redb transaction error: {0}")]
    TransactionError(#[from] redb::TransactionError),
    /// redb table error
    #[error("redb table error: {0}")]
    TableError(#[from] redb::TableError),
    /// redb storage error
    #[error("redb storage error: {0}")]
    StorageError(#[from] redb::StorageError),
    /// redb commit error
    #[error("redb storage error: {0}")]
    CommitError(#[from] redb::CommitError),
    /// error formatting time
    #[error("error formatting time: {0}")]
    TimeFormatError(#[from] time::error::Format),
    /// error parsing time
    #[error("error parsing time: {0}")]
    TimeParseError(#[from] time::error::Parse),
    /// error creating directory for database
    #[error("error creating directory for database: {0}")]
    CreateDbDirError(std::io::Error),
}

/// The Clap type for all the commandline parameters
#[derive(clap::Parser, Debug)]
#[clap(name = clap::crate_name!(),
       about = clap::crate_description!(),
       author = clap::crate_authors!(),
       version = clap::crate_version!(),
       )]
struct Options {
    /// name of the logged in avatar whose chat.txt log file to watch (not display name)
    #[clap(long)]
    avatar_name: String,
}

/// a wrapped error in case parsing fails to get proper error output
/// the chumsky errors themselves lack Display and std::error::Error
/// implementations
#[derive(Debug)]
pub struct ChumskyError {
    /// description of the object we were trying to parse
    pub description: String,
    /// source string for parsing
    pub source: String,
    /// errors encountered during parsing
    pub errors: Vec<chumsky::error::Simple<char>>,
}

impl std::fmt::Display for ChumskyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for e in &self.errors {
            let msg = format!(
                "While parsing {}: {}{}, expected {}",
                self.description,
                if e.found().is_some() {
                    "Unexpected token"
                } else {
                    "Unexpected end of input"
                },
                if let Some(label) = e.label() {
                    format!(" while parsing {}", label)
                } else {
                    String::new()
                },
                if e.expected().len() == 0 {
                    "end of input".to_string()
                } else {
                    e.expected()
                        .map(|expected| match expected {
                            Some(expected) => expected.to_string(),
                            None => "end of input".to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );

            let report = Report::build(ReportKind::Error, e.span())
                .with_code(3)
                .with_message(msg)
                .with_label(
                    Label::new(e.span())
                        .with_message(format!(
                            "Unexpected {}",
                            e.found()
                                .map(|c| format!("token {}", c.fg(Color::Red)))
                                .unwrap_or_else(|| "end of input".to_string())
                        ))
                        .with_color(Color::Red),
                );

            let report = match e.reason() {
                chumsky::error::SimpleReason::Unclosed { span, delimiter } => report.with_label(
                    Label::new(span.clone())
                        .with_message(format!(
                            "Unclosed delimiter {}",
                            delimiter.fg(Color::Yellow)
                        ))
                        .with_color(Color::Yellow),
                ),
                chumsky::error::SimpleReason::Unexpected => report,
                chumsky::error::SimpleReason::Custom(msg) => report.with_label(
                    Label::new(e.span())
                        .with_message(format!("{}", msg.fg(Color::Yellow)))
                        .with_color(Color::Yellow),
                ),
            };

            let mut s: Vec<u8> = Vec::new();
            report
                .finish()
                .write(Source::from(&self.source), &mut s)
                .map_err(|_| <std::fmt::Error as std::default::Default>::default())?;
            let Ok(s) = std::str::from_utf8(&s) else {
                tracing::error!("Expected ariadne to produce valid UTF-8");
                return Err(std::fmt::Error);
            };
            write!(f, "{}", s)?;
        }
        Ok(())
    }
}

impl std::error::Error for ChumskyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// determine avatar log dir from avatar name
fn avatar_log_dir(avatar_name: &str) -> Result<PathBuf, crate::Error> {
    let avatar_dir_name = avatar_name.replace(' ', "_").to_lowercase();
    tracing::debug!("Avatar dir name: {}", avatar_dir_name);

    let Some(home_dir) = dirs2::home_dir() else {
        tracing::error!("Could not determine current user home directory");
        return Err(crate::Error::HomeDirError);
    };

    Ok(home_dir.join(".firestorm/").join(avatar_dir_name))
}

/// parse a chat line as a welcome greeting and return the names of the greeted people
///
/// # Errors
///
/// returns an error if the parser fails
fn welcome_greeting_parser() -> impl Parser<char, Vec<String>, Error = Simple<char>> {
    just("hi")
        .or(just("hello"))
        .or(just("hallo"))
        .or(just("ahoy"))
        .or(just("wb"))
        .or(just("welcome back"))
        .ignore_then(whitespace())
        .ignore_then(
            take_until(
                just(",")
                    .or(just("and"))
                    .or(just("und"))
                    .or(just("\n").or(end().map(|_| "")))
                    .rewind(),
            )
            .separated_by(just(",").or(just("and")).or(just("und")).or(just("\n"))),
        )
        .try_map(|s, _span: std::ops::Range<usize>| {
            Ok(s.into_iter()
                .map(|(s, _)| s.into_iter().collect::<String>().trim().to_string())
                .collect())
        })
}

/// write last seen timestamp to redb database
fn write_last_seen_to_db(
    db: &redb::Database,
    name: &str,
    timestamp: &time::PrimitiveDateTime,
) -> Result<(), crate::Error> {
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(LAST_SEEN_TABLE)?;
        table.insert(name.to_lowercase(), &timestamp.format(TIME_FORMAT)?)?;
    }
    write_txn.commit()?;
    Ok(())
}

/// The main behaviour of the binary should go here
#[instrument]
async fn do_stuff() -> Result<(), crate::Error> {
    let options = <Options as clap::Parser>::parse();
    tracing::debug!("{:#?}", options);

    let Some(db_path) = dirs2::config_dir() else {
        return Err(crate::Error::CouldNotDetermineDatabaseStorageDir);
    };
    let db_path = db_path.join(clap::crate_name!());
    let db_path = db_path.join(&options.avatar_name);
    std::fs::create_dir_all(&db_path).map_err(crate::Error::CreateDbDirError)?;

    let db = redb::Database::create(db_path.join("last_seen.redb"))?;

    let avatar_dir = avatar_log_dir(&options.avatar_name)?;

    let local_chat_log_file = avatar_dir.join("chat.txt");

    if !local_chat_log_file.exists() {
        tracing::error!(
            "Local chat log {} does not exist for this avatar",
            local_chat_log_file.display()
        );
        return Err(crate::Error::LocalChatFileNotFound(local_chat_log_file));
    }

    let mut lines = linemux::MuxedLines::new().map_err(crate::Error::MuxedLinesError)?;

    lines
        .add_file(local_chat_log_file)
        .await
        .map_err(crate::Error::MuxedLinesAddFileError)?;

    let mut last_line: Option<String> = None;

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    let join_handle = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Err(e) = tx.send(line).await {
                tracing::error!("Error sending line: {:?}", e);
            }
        }
    });

    let (tx2, mut rx2) = tokio::sync::mpsc::channel(16);

    let join_handle2 = tokio::spawn(async move {
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(1), rx.recv()).await {
                Err(tokio::time::error::Elapsed { .. }) => {
                    if let Some(ref ll) = last_line {
                        if let Err(e) = tx2.send(ll.clone()).await {
                            tracing::error!("Error sending line (tx2): {:?}", e);
                        }
                        last_line = None;
                    }
                }
                Ok(Some(line)) => {
                    last_line = if let Some(ref ll) = last_line {
                        if line.line().starts_with(' ') || line.line() == "" {
                            Some(format!("{}\n{}", ll, line.line()))
                        } else {
                            if let Err(e) = tx2.send(ll.clone()).await {
                                tracing::error!("Error sending line (tx2): {:?}", e);
                            }
                            Some(line.line().to_string())
                        }
                    } else {
                        Some(line.line().to_string())
                    };
                }
                _ => {
                    break;
                }
            }
        }
    });

    let mut notify_handles: BTreeMap<String, notify_rust::NotificationHandle> = BTreeMap::new();
    let mut last_seen_in_chat_range: BTreeMap<String, time::PrimitiveDateTime> = BTreeMap::new();

    {
        let read_txn = db.begin_read()?;
        if let Ok(table) = read_txn.open_table(LAST_SEEN_TABLE) {
            let _ = table.iter().map(|mut range| {
                for item in range.by_ref() {
                    let (key, value) = item?;
                    let name = key.value();
                    let timestamp = value.value();
                    let timestamp = time::PrimitiveDateTime::parse(&timestamp, &TIME_FORMAT)?;
                    last_seen_in_chat_range.insert(name, timestamp);
                }
                Ok::<(), crate::Error>(())
            })?;
        }
    }

    while let Some(line) = rx2.recv().await {
        println!("parsing line:\n{}", line);
        let parsed_line = sl_chat_log_parser::chat_log_line_parser().parse(line.clone());
        println!("parse result:\n{:#?}", parsed_line);

        if let Ok(sl_chat_log_parser::ChatLogLine {
            timestamp,
            event:
                sl_chat_log_parser::ChatLogEvent::AvatarLine {
                    ref name,
                    message:
                        sl_chat_log_parser::avatar_messages::AvatarMessage::EnteredArea {
                            area: sl_types::radar::Area::ChatRange,
                            distance: _,
                        },
                },
        }) = parsed_line
        {
            let (last_seen_description, last_seen_age) = if let Some(last_seen_timestamp) =
                last_seen_in_chat_range.get(&name.to_lowercase())
            {
                if let Some(timestamp) = timestamp {
                    let last_seen_age = timestamp - *last_seen_timestamp;
                    if let Ok(std_last_seen_age) = last_seen_age.try_into() {
                        (
                            format!(
                                "Last seen {} ago ({})",
                                <humantime::Duration as From<std::time::Duration>>::from(
                                    std_last_seen_age
                                ),
                                last_seen_timestamp
                            ),
                            Some(last_seen_age),
                        )
                    } else {
                        (
                            format!(
                                "Could not convert last seen age to humantime: {}",
                                last_seen_age
                            ),
                            Some(last_seen_age),
                        )
                    }
                } else {
                    (
                        "Unable to determine timestamp for current message".to_string(),
                        None,
                    )
                }
            } else {
                ("Not seen recently".to_string(), None)
            };
            if last_seen_age.is_none()
                || last_seen_age
                    .is_some_and(|last_seen_age| last_seen_age > std::time::Duration::from_secs(5))
            {
                match notify_rust::Notification::new()
                    .appname("sl-hello-goodbye")
                    .summary("New person entered chat range")
                    .body(&format!(
                        "{} entered the chat range\n{}",
                        name, last_seen_description
                    ))
                    .hint(notify_rust::Hint::Resident(true))
                    .timeout(notify_rust::Timeout::Never)
                    .show()
                {
                    Ok(notify_handle) => {
                        notify_handles.insert(name.to_string().to_lowercase(), notify_handle);
                    }
                    Err(e) => {
                        tracing::error!("Error sending notification: {:?}", e);
                    }
                }
            }
            if let Some(timestamp) = timestamp {
                last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
                write_last_seen_to_db(&db, name, &timestamp)?;
            }
        }

        if let Ok(sl_chat_log_parser::ChatLogLine {
            timestamp,
            event:
                sl_chat_log_parser::ChatLogEvent::AvatarLine {
                    ref name,
                    message:
                        sl_chat_log_parser::avatar_messages::AvatarMessage::LeftArea {
                            area: sl_types::radar::Area::ChatRange,
                        },
                },
        }) = parsed_line
        {
            if let Some(timestamp) = timestamp {
                last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
                write_last_seen_to_db(&db, name, &timestamp)?;
            }
            let name = name.to_lowercase();
            let mut to_remove = Vec::new();
            for n in notify_handles.keys() {
                if *n == name {
                    to_remove.push(name.to_string());
                }
            }
            for name in to_remove {
                if let Some(notify_handle) = notify_handles.remove(&name) {
                    notify_handle.close();
                }
            }
        }

        // TODO:
        // leave announcements and left chat range
        // Examples
        // "Take care all"
        // "RL is calling me"
        // "I have to go"
        // "I have to head out"
        // "I have to take off"
        // "(Good)bye everyone"
        // "(Good)bye everybody"
        // "(Good)bye all"
        // "Dinnertime for me"
        // "I have to get some sleep"
        // "It is my bedtime"
        // "Gotta go"
        // "Good night all"
        // "I am going to call it a day"
        // "I don't feel so good"
        // "I am going to lie down"
        // "I am going to get some rest"
        // "I have to get up early"
        // (abbreviated versions like tc for take care, gn for good night)
        // (other people saying good bye or good night to someone or telling them to take care, sweet dreams, sleep well, have a good rest)
        // (though that might also be the person leaving saying good bye to specific people)
        //
        // relog or afk announcements and welcome back
        // "I have to relog"
        // "relog, brb"
        // "afk"
        // "brb"
        //
        // "back"

        if let Ok(sl_chat_log_parser::ChatLogLine {
            timestamp,
            event:
                sl_chat_log_parser::ChatLogEvent::AvatarLine {
                    ref name,
                    message:
                        sl_chat_log_parser::avatar_messages::AvatarMessage::Chat {
                            ref message,
                            volume,
                        },
                },
        }) = parsed_line
        {
            if *name == options.avatar_name {
                if let Ok(greeted) = welcome_greeting_parser().parse(message.to_lowercase()) {
                    tracing::debug!("Found welcoming greeting greeting\n{:#?}", greeted);
                    for greeted in greeted {
                        let greeted = greeted.to_lowercase();
                        let mut to_remove = Vec::new();
                        for name in notify_handles.keys() {
                            if name.contains(&greeted) {
                                to_remove.push(name.to_string());
                            }
                        }
                        for name in to_remove {
                            if let Some(notify_handle) = notify_handles.remove(&name) {
                                notify_handle.close();
                            }
                        }
                    }
                }
            } else if let Some(timestamp) = timestamp
                && volume <= sl_types::chat::ChatVolume::Say
            {
                last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
                write_last_seen_to_db(&db, name, &timestamp)?;
            }
        }

        if let Ok(sl_chat_log_parser::ChatLogLine {
            timestamp: Some(timestamp),
            event:
                sl_chat_log_parser::ChatLogEvent::AvatarLine {
                    name,
                    message:
                        sl_chat_log_parser::avatar_messages::AvatarMessage::Emote { message: _, volume },
                },
        }) = parsed_line
            && volume <= sl_types::chat::ChatVolume::Say
        {
            last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
            write_last_seen_to_db(&db, &name, &timestamp)?;
        }
    }

    join_handle.await?;
    join_handle2.await?;

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
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_welcome_greeting_parser_one_avatar() -> Result<(), Error> {
        match welcome_greeting_parser().parse("hello john") {
            Ok(parsed) => {
                assert_eq!(parsed, ["john"]);
            }
            Err(e) => {
                for err in &e {
                    tracing::error!("{}", err);
                }
                return Err(crate::Error::ChatLogLineParseError(ChumskyError {
                    description: "welcome greeting".to_string(),
                    source: "hello john".to_string(),
                    errors: e,
                }));
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_welcome_greeting_parser_two_avatars() -> Result<(), Error> {
        match welcome_greeting_parser().parse("hello john and paul") {
            Ok(parsed) => {
                assert_eq!(parsed, ["john", "paul"]);
            }
            Err(e) => {
                for err in &e {
                    tracing::error!("{}", err);
                }
                return Err(crate::Error::ChatLogLineParseError(ChumskyError {
                    description: "welcome greeting two avatars".to_string(),
                    source: "hello john and paul".to_string(),
                    errors: e,
                }));
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_welcome_greeting_parser_three_avatars() -> Result<(), Error> {
        match welcome_greeting_parser().parse("hello john, paul and mary") {
            Ok(parsed) => {
                assert_eq!(parsed, ["john", "paul", "mary"]);
            }
            Err(e) => {
                for err in &e {
                    tracing::error!("{}", err);
                }
                return Err(crate::Error::ChatLogLineParseError(ChumskyError {
                    description: "welcome greeting three avatars".to_string(),
                    source: "hello john, paul and mary".to_string(),
                    errors: e,
                }));
            }
        }
        Ok(())
    }
}
