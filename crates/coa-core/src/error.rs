use serde::Serialize;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("path rejected: {0}")]
    PathRejected(String),
    #[error("not enough free space: need {needed} bytes, have {available}")]
    InsufficientSpace { needed: u64, available: u64 },
    #[error("hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch { path: String, expected: String, actual: String },
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("installation {0} is not registered")]
    UnknownInstallation(String),
    #[error("{0}")]
    Invalid(String),
    #[error("package not published: {0}")]
    PackageNotPublished(String),
    #[error("download server unreachable: {0}")]
    NetworkUnreachable(String),
    #[error("the bot module found no level 80 template characters")]
    CompanionTemplatesMissing,
    #[error("CoA Companions and SQUID Playerbots cannot both be enabled. Disable one bot module.")]
    BotModulesConflict,
    #[error("this server's bot module does not have that command yet")]
    CompanionCommandUnsupported,
    #[error("invalid values: {}", .0.iter().map(|f| format!("{}: {}", f.key, f.message)).collect::<Vec<_>>().join("; "))]
    Validation(Vec<FieldError>),
}

/// A problem with one setting, addressed by key so the UI can highlight the field.
#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub key: String,
    pub message: String,
}

/// Stable identifiers the UI maps to human messages. Raw exit codes never reach the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    DatabaseNotRunning,
    PortInUse,
    ServerFilesIncomplete,
    WorldDbIncompatible,
    WorldserverAlreadyRunning,
    InvalidConfigValue,
    DiskFull,
    OperationInProgress,
    StartupFailed,
    DockerUnavailable,
    GameDataMismatch,
    HashMismatch,
    PathRejected,
    PackageNotPublished,
    NetworkUnreachable,
    CompanionTemplatesMissing,
    BotModulesConflict,
    CompanionCommandUnsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FixAction {
    FixAutomatically,
    OpenFolder,
    ShowDetails,
    ChooseAnotherFolder,
    Retry,
}

#[derive(Debug, Clone, Serialize)]
pub struct Human {
    pub code: ErrorCode,
    pub title: &'static str,
    pub message: &'static str,
    pub actions: &'static [FixAction],
}

impl ErrorCode {
    pub fn human(self) -> Human {
        use FixAction::*;
        let (title, message, actions): (&str, &str, &'static [FixAction]) = match self {
            ErrorCode::BotModulesConflict => ("Two bot modules are enabled", "Disable CoA Companions or SQUID Playerbots in Modules before starting the server.", &[ShowDetails]),
            ErrorCode::DatabaseNotRunning => (
                "Database isn't running",
                "The server needs its database to start. Start the database first.",
                &[FixAutomatically, ShowDetails],
            ),
            ErrorCode::PortInUse => (
                "Server port is already in use",
                "Another program (or another server) is using a port this server needs.",
                &[ShowDetails],
            ),
            ErrorCode::ServerFilesIncomplete => (
                "Server files are incomplete",
                "Some server files are missing or damaged.",
                &[FixAutomatically, OpenFolder, ShowDetails],
            ),
            ErrorCode::WorldDbIncompatible => (
                "World database is incompatible with this build",
                "A database update is required before this server version can start.",
                &[FixAutomatically, ShowDetails],
            ),
            ErrorCode::WorldserverAlreadyRunning => (
                "Another worldserver process is already running",
                "Stop the other server before starting this one.",
                &[ShowDetails],
            ),
            ErrorCode::InvalidConfigValue => (
                "Your server configuration contains an invalid value",
                "Fix the highlighted setting and save again.",
                &[OpenFolder, ShowDetails],
            ),
            ErrorCode::DiskFull => (
                "Not enough free disk space",
                "Free up some space or choose another drive, then try again.",
                &[ChooseAnotherFolder, Retry],
            ),
            ErrorCode::OperationInProgress => (
                "Another action is still in progress",
                "The server is busy starting or stopping. Wait a moment and try again.",
                &[Retry],
            ),
            ErrorCode::StartupFailed => (
                "Server could not start",
                "A service stopped while starting. Check the details for the cause.",
                &[FixAutomatically, ShowDetails],
            ),
            ErrorCode::DockerUnavailable => (
                "Docker is not available",
                "This server runs in Docker. Install Docker, start it, and make sure your user account may use it, then try again.",
                &[Retry, ShowDetails],
            ),
            ErrorCode::GameDataMismatch => (
                "The game data folder is not the CoA set",
                "The server stopped because the DBC files in the game data folder are not the ones of the Conquest of Azeroth client. Choose the folder that holds the CoA data (Settings > Game data folder), then start again.",
                &[ShowDetails],
            ),
            ErrorCode::HashMismatch => (
                "A downloaded file is damaged",
                "The file did not match its checksum and was not used.",
                &[Retry, ShowDetails],
            ),
            ErrorCode::PathRejected => (
                "That location is not allowed",
                "The requested file location is outside the server folder.",
                &[ShowDetails],
            ),
            ErrorCode::PackageNotPublished => (
                "This server package is not available yet",
                "Nothing has been published at the download location. Try again later, or install from a local package folder (Advanced).",
                &[Retry, ShowDetails],
            ),
            ErrorCode::NetworkUnreachable => (
                "Could not reach the download server",
                "Check your internet connection and try again.",
                &[Retry, ShowDetails],
            ),
            ErrorCode::CompanionTemplatesMissing => (
                "Companions have nothing to be copied from yet",
                "New companions are copied from level 80 characters of each class. Create one level 80 character per class on any account except the console account, then try again.",
                &[ShowDetails],
            ),
            ErrorCode::CompanionCommandUnsupported => (
                "This server version can not do that yet",
                "The bot module of this server build has no such command. Update the server, or restart it to stop what is running.",
                &[ShowDetails],
            ),
            ErrorCode::Unknown => ("Something went wrong", "See the technical details.", &[ShowDetails]),
        };
        Human { code: self, title, message, actions }
    }
}

impl Error {
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::InsufficientSpace { .. } => ErrorCode::DiskFull,
            Error::HashMismatch { .. } => ErrorCode::HashMismatch,
            Error::PathRejected(_) => ErrorCode::PathRejected,
            Error::Validation(_) => ErrorCode::InvalidConfigValue,
            Error::PackageNotPublished(_) => ErrorCode::PackageNotPublished,
            Error::NetworkUnreachable(_) => ErrorCode::NetworkUnreachable,
            Error::BotModulesConflict => ErrorCode::BotModulesConflict,
            Error::CompanionTemplatesMissing => ErrorCode::CompanionTemplatesMissing,
            Error::CompanionCommandUnsupported => ErrorCode::CompanionCommandUnsupported,
            _ => ErrorCode::Unknown,
        }
    }
}

/// Error shape sent over Tauri IPC: human text plus the technical string (needed for GitHub issues).
#[derive(Debug, Serialize)]
pub struct UiError {
    pub human: Human,
    pub technical: String,
    pub fields: Vec<FieldError>,
}

impl From<Error> for UiError {
    fn from(e: Error) -> Self {
        let fields = match &e {
            Error::Validation(f) => f.clone(),
            _ => Vec::new(),
        };
        UiError { human: e.code().human(), technical: e.to_string(), fields }
    }
}
