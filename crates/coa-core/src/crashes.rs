//! Crash log discovery and human-friendly explanation for worldserver crashes.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrashItem {
    pub id: String,
    pub timestamp_ms: u64,
    pub date_str: String,
    pub title: String,
    pub explanation: String,
    pub category: String,
    pub exception_code: Option<String>,
    pub location: Option<String>,
    pub function: Option<String>,
    pub condition: Option<String>,
    pub preview: String,
    pub full_log: String,
}

/// Lists recent crash reports from `Core/Crashes`, sorted newest first.
pub fn list(root: &Path, limit: usize) -> Result<Vec<CrashItem>> {
    let dir = root.join("Core/Crashes");
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("txt") {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((path, mtime));
            }
        }
    }
    files.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    files.truncate(limit.max(1));

    let mut items = Vec::new();
    for (path, mtime) in files {
        if let Ok(content) = fs::read_to_string(&path) {
            let item = parse_crash_content(&path, mtime, &content);
            items.push(item);
        }
    }
    Ok(items)
}

/// Parses crash report content and generates human-friendly titles and explanations.
pub fn parse_crash_content(path: &Path, mtime: SystemTime, content: &str) -> CrashItem {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let timestamp_ms = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut date_str = String::new();
    let mut exception_code = None;
    let mut location = None;
    let mut function = None;
    let mut condition = None;

    let lines: Vec<&str> = content.lines().collect();

    let mut in_assertion = false;
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("Date ") && date_str.is_empty() {
            date_str = trimmed.to_string();
        } else if trimmed.starts_with("Exception code:") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                exception_code = Some(parts[2].to_string());
            }
        } else if trimmed.contains(">> ASSERTION FAILED") {
            in_assertion = true;
        } else if in_assertion {
            if trimmed.starts_with("# Location:") {
                let loc = trimmed.trim_start_matches("# Location:").trim();
                let clean = loc.rsplit(['\\', '/']).take(2).collect::<Vec<_>>();
                location = Some(if clean.len() == 2 {
                    format!("{}/{}", clean[1], clean[0])
                } else {
                    loc.to_string()
                });
            } else if trimmed.starts_with("# Function:") {
                function = Some(trimmed.trim_start_matches("# Function:").trim().to_string());
            } else if trimmed.starts_with("# Condition:") {
                condition = Some(
                    trimmed
                        .trim_start_matches("# Condition:")
                        .trim()
                        .to_string(),
                );
            } else if trimmed.starts_with("#---") && condition.is_some() {
                in_assertion = false;
            }
        }
    }

    if date_str.is_empty() {
        if let Some(pos) = filename.find("_[") {
            if let Some(end) = filename[pos..].find(']') {
                date_str = filename[pos + 2..pos + end].replace('_', " ");
            }
        }
    }

    let (category, title, explanation) = explain(
        exception_code.as_deref(),
        function.as_deref(),
        condition.as_deref(),
        content,
    );

    let preview_lines: Vec<&str> = lines.iter().take(25).copied().collect();
    let preview = preview_lines.join("\n");

    CrashItem {
        id: filename,
        timestamp_ms,
        date_str,
        title,
        explanation,
        category,
        exception_code,
        location,
        function,
        condition,
        preview,
        full_log: content.to_string(),
    }
}

fn explain(
    code: Option<&str>,
    func: Option<&str>,
    cond: Option<&str>,
    full: &str,
) -> (String, String, String) {
    let lower_full = full.to_lowercase();
    let lower_func = func.map(|f| f.to_lowercase()).unwrap_or_default();
    let lower_cond = cond.map(|c| c.to_lowercase()).unwrap_or_default();

    if let Some(c) = code {
        if c.eq_ignore_ascii_case("C0000420") {
            if lower_func.contains("removefromgrid") || lower_cond.contains("isingrid") {
                return (
                    "assertion".into(),
                    "Сбой удаления сущности из сетки карты (Grid / Despawn)".into(),
                    "Сервер попытался выгрузить объект (игрока, NPC или бота), который уже не находился в сетке карты. Обычно происходит при резком разрыве соединения, телепортации или одновременном деспавне нескольких сущностей.".into(),
                );
            }
            if lower_func.contains("map::") || lower_func.contains("grid::") {
                return (
                    "assertion".into(),
                    "Ошибка обновления сетки игрового мира (Map Grid)".into(),
                    "Сбой при загрузке или обновлении клеток локации (Grid/Cell). Может быть вызван перемещением сущностей через границы чанков.".into(),
                );
            }
            if lower_func.contains("playerbot") || lower_func.contains("aiplayerbot") {
                return (
                    "assertion".into(),
                    "Ошибка в модуле ботов (Playerbots)".into(),
                    "Внутренний сбой в логике ИИ ботов (экипировка, заклинание или поиск пути)."
                        .into(),
                );
            }
            let cond_label = cond.unwrap_or("Assert condition failed");
            return (
                "assertion".into(),
                format!("Сработала внутренняя проверка ядра ({cond_label})"),
                format!("Сервер обнаружил нарушение целостности данных в функции {}. Выполнение было аварийно остановлено.", func.unwrap_or("неизвестно")),
            );
        } else if c.eq_ignore_ascii_case("C0000005") {
            return (
                "access_violation".into(),
                "Нарушение доступа к памяти (Access Violation)".into(),
                "Попытка чтения или записи по некорректному адресу памяти (разыменование null-указателя или освобождённого объекта).".into(),
            );
        } else if c.eq_ignore_ascii_case("C00000FD") {
            return (
                "stack_overflow".into(),
                "Переполнение стека (Stack Overflow)".into(),
                "Исчерпание стека вызовов, вызванное бесконечной рекурсией или слишком глубоким стеком вызовов функций.".into(),
            );
        } else if c.eq_ignore_ascii_case("E06D7363") {
            if lower_full.contains("bad_alloc") {
                return (
                    "out_of_memory".into(),
                    "Нехватка оперативной памяти (Out of Memory)".into(),
                    "Серверу не удалось выделить оперативную память (std::bad_alloc). Проверьте объем доступной RAM.".into(),
                );
            }
            return (
                "exception".into(),
                "Необработанное исключение C++ (C++ Exception)".into(),
                "Произошло критическое исключение C++, которое не было перехвачено ядром сервера."
                    .into(),
            );
        }
    }

    if lower_full.contains("mysql server has gone away") || lower_full.contains("connection lost") {
        return (
            "database".into(),
            "Потеряна связь с базой данных (MySQL)".into(),
            "Связь сервера с СУБД MySQL была прервана или служба базы данных упала.".into(),
        );
    }

    if lower_full.contains("address already in use") || lower_full.contains("bind failed") {
        return (
            "network".into(),
            "Сетевой порт уже занят".into(),
            "Порт мира, авторизации или базы данных уже занят другим приложением.".into(),
        );
    }

    (
        "generic".into(),
        "Непредвиденный сбой процесса сервера".into(),
        "Процесс worldserver аварийно завершился. Детали и стек вызовов доступны в полном тексте отчёта.".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assertion_report() {
        let report = r#"Revision: AzerothCore rev. df5bcdf26092 2026-10-02 22:08:42 +0500 (coa-bots branch) (Win64, RelWithDebInfo, Static)
Date 3:10:2026. Time 20:20 
//=====================================================
Exception code: C0000420 Ein Assertionsfehler ist aufgetreten.

Assertion message: #----------------------------------------------------------------------#
 
>> ASSERTION FAILED

# Location: D:\a\coa-server-build\src\server\game\Entities\Object\Object.h:376
# Function: GridObject<class Player>::RemoveFromGrid
# Condition: IsInGrid()
 
#----------------------------------------------------------------------#

Fault address:  00007FF7ECD3645E
"#;
        let item = parse_crash_content(
            Path::new("dummy_worldserver.exe_[3-10_20-20-51].txt"),
            SystemTime::UNIX_EPOCH,
            report,
        );
        println!("ITEM: {item:#?}");
        assert_eq!(item.exception_code.as_deref(), Some("C0000420"));
        assert_eq!(item.category, "assertion");
        assert_eq!(item.condition.as_deref(), Some("IsInGrid()"));
        assert_eq!(
            item.function.as_deref(),
            Some("GridObject<class Player>::RemoveFromGrid")
        );
        assert!(item.title.contains("Grid"));
        assert!(item.explanation.contains("сетке карты"));
    }

    #[test]
    fn parses_access_violation() {
        let report = r#"Date 4:10:2026. Time 12:00
Exception code: C0000005 ACCESS_VIOLATION
Fault address: 00007FF7ECD3645E
"#;
        let item = parse_crash_content(Path::new("crash.txt"), SystemTime::UNIX_EPOCH, report);
        assert_eq!(item.exception_code.as_deref(), Some("C0000005"));
        assert_eq!(item.category, "access_violation");
        assert!(item.title.contains("Access Violation"));
    }
}
