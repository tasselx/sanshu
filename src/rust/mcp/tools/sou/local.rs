use anyhow::{anyhow, Context, Result};
use ignore::WalkBuilder;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::Lazy;
use ring::digest::{Context as ShaContext, SHA256};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

const INDEX_MISSING: u8 = 0;
const INDEX_BUILDING: u8 = 1;
const INDEX_READY: u8 = 2;
const INDEX_ERROR: u8 = 3;
const CHUNK_LINES: usize = 80;
const CHUNK_OVERLAP: usize = 20;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_QUERY_TERMS: usize = 24;

static PROJECT_INDEXES: Lazy<Mutex<HashMap<PathBuf, Arc<ProjectIndex>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub(super) struct LocalSearchOptions {
    pub project_root: PathBuf,
    pub query: String,
    pub max_results: usize,
    pub exclude_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct LocalSearchOutput {
    pub text: String,
    pub hit_count: usize,
    pub engine: String,
    pub index_state: String,
    pub fallback_reason: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalIndexStatus {
    pub project_root: String,
    pub index_path: String,
    pub state: String,
    pub indexed_files: u64,
    pub indexed_chunks: u64,
    pub sync_running: bool,
    pub pending_changes: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct SearchHit {
    relative_path: String,
    start_line: usize,
    end_line: usize,
    excerpt: String,
    coverage: usize,
    exact_match: bool,
    path_matches: usize,
    lexical_score: f64,
}

struct ProjectIndex {
    root: PathBuf,
    db_path: PathBuf,
    state: AtomicU8,
    indexed_files: AtomicU64,
    indexed_chunks: AtomicU64,
    sync_running: AtomicBool,
    dirty: Arc<AtomicBool>,
    profile_hash: Mutex<String>,
    last_error: Mutex<Option<String>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
}

impl ProjectIndex {
    fn new(root: PathBuf, db_path: PathBuf) -> Self {
        let (state, files, chunks, error) = inspect_existing_index(&db_path);
        Self {
            root,
            db_path,
            state: AtomicU8::new(state),
            indexed_files: AtomicU64::new(files),
            indexed_chunks: AtomicU64::new(chunks),
            sync_running: AtomicBool::new(false),
            // 进程重启后先做一次元数据对账，查询仍可读取已有索引。
            dirty: Arc::new(AtomicBool::new(state == INDEX_READY)),
            profile_hash: Mutex::new(String::new()),
            last_error: Mutex::new(error),
            watcher: Mutex::new(None),
        }
    }

    fn state_name(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            INDEX_BUILDING => "building",
            INDEX_READY => "ready",
            INDEX_ERROR => "error",
            _ => "missing",
        }
    }

    fn ensure_watcher(&self) -> Result<()> {
        let mut guard = self
            .watcher
            .lock()
            .map_err(|_| anyhow!("本地索引 watcher 锁已损坏"))?;
        if guard.is_some() {
            return Ok(());
        }

        let dirty = Arc::clone(&self.dirty);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(_) => {
                    dirty.store(true, Ordering::Release);
                }
                Err(error) => log::warn!("[sou-local] 文件监听事件失败: {}", error),
            })
            .context("创建本地索引文件监听器失败")?;
        watcher
            .watch(&self.root, RecursiveMode::Recursive)
            .with_context(|| format!("监听项目目录失败: {}", self.root.display()))?;
        *guard = Some(watcher);
        Ok(())
    }

    fn status(&self) -> LocalIndexStatus {
        LocalIndexStatus {
            project_root: normalize_path(&self.root),
            index_path: normalize_path(&self.db_path),
            state: self.state_name().to_string(),
            indexed_files: self.indexed_files.load(Ordering::Acquire),
            indexed_chunks: self.indexed_chunks.load(Ordering::Acquire),
            sync_running: self.sync_running.load(Ordering::Acquire),
            pending_changes: self.dirty.load(Ordering::Acquire),
            last_error: self.last_error.lock().ok().and_then(|value| value.clone()),
        }
    }
}

pub(super) async fn search(options: LocalSearchOptions) -> Result<LocalSearchOutput> {
    let root = options
        .project_root
        .canonicalize()
        .with_context(|| format!("本地搜索项目路径无效: {}", options.project_root.display()))?;
    if !root.is_dir() {
        return Err(anyhow!("本地搜索项目路径不是目录: {}", root.display()));
    }

    let index = project_index(&root)?;
    search_with_index(options, root, index, true).await
}

#[cfg(test)]
pub(super) async fn search_for_test(
    options: LocalSearchOptions,
    index_path: PathBuf,
) -> Result<LocalSearchOutput> {
    let root = options
        .project_root
        .canonicalize()
        .with_context(|| format!("本地搜索项目路径无效: {}", options.project_root.display()))?;
    if !root.is_dir() {
        return Err(anyhow!("本地搜索项目路径不是目录: {}", root.display()));
    }

    let index = Arc::new(ProjectIndex::new(root.clone(), index_path));
    sync_now(Arc::clone(&index), options.exclude_paths.clone()).await?;
    search_with_index(options, root, index, false).await
}

async fn search_with_index(
    options: LocalSearchOptions,
    root: PathBuf,
    index: Arc<ProjectIndex>,
    enable_watcher: bool,
) -> Result<LocalSearchOutput> {
    let started_at = Instant::now();
    let terms = extract_query_terms(&options.query);
    if terms.is_empty() {
        return Err(anyhow!("本地搜索未提取到有效关键词"));
    }

    if enable_watcher {
        if let Err(error) = index.ensure_watcher() {
            log::warn!(
                "[sou-local] watcher 启动失败，继续使用查询时对账: {}",
                error
            );
        }
    }
    refresh_profile(&index, &options.exclude_paths);

    let mut fallback_reason = None;
    let (hits, engine) = if index.state.load(Ordering::Acquire) == INDEX_READY {
        if index.dirty.load(Ordering::Acquire) || index.sync_running.load(Ordering::Acquire) {
            fallback_reason = Some("本地索引存在待同步变更，本次使用即时搜索".to_string());
            schedule_sync(Arc::clone(&index), options.exclude_paths.clone());
            run_immediate_search(&root, &options, &terms).await?
        } else {
            let db_path = index.db_path.clone();
            let query = options.query.clone();
            let query_terms = terms.clone();
            let max_results = options.max_results;
            match tokio::task::spawn_blocking(move || {
                query_index(&db_path, &query, &query_terms, max_results)
            })
            .await
            .context("等待 FTS5 查询任务失败")?
            {
                Ok(hits) => (hits, "fts5".to_string()),
                Err(error) => {
                    let reason = format!("FTS5 查询失败: {}", error);
                    mark_index_error(&index, &reason);
                    schedule_sync(Arc::clone(&index), options.exclude_paths.clone());
                    fallback_reason = Some(reason);
                    run_immediate_search(&root, &options, &terms).await?
                }
            }
        }
    } else {
        let state = index.state_name().to_string();
        fallback_reason = Some(format!("本地索引状态为 {}", state));
        schedule_sync(Arc::clone(&index), options.exclude_paths.clone());
        run_immediate_search(&root, &options, &terms).await?
    };

    let duration_ms = started_at.elapsed().as_millis() as u64;
    let state = index.state_name().to_string();
    let text = format_hits(
        &root,
        &hits,
        &engine,
        &state,
        duration_ms,
        fallback_reason.as_deref(),
    );
    Ok(LocalSearchOutput {
        text,
        hit_count: hits.len(),
        engine,
        index_state: state,
        fallback_reason,
        duration_ms,
    })
}

pub async fn rebuild(project_root: &str, exclude_paths: Vec<String>) -> Result<LocalIndexStatus> {
    let root = PathBuf::from(project_root)
        .canonicalize()
        .with_context(|| format!("本地索引项目路径无效: {}", project_root))?;
    let index = project_index(&root)?;
    index.dirty.store(false, Ordering::Release);
    sync_now(Arc::clone(&index), exclude_paths).await?;
    Ok(index.status())
}

pub fn status(project_root: &str) -> Result<LocalIndexStatus> {
    let root = PathBuf::from(project_root)
        .canonicalize()
        .with_context(|| format!("本地索引项目路径无效: {}", project_root))?;
    Ok(project_index(&root)?.status())
}

fn project_index(root: &Path) -> Result<Arc<ProjectIndex>> {
    let mut indexes = PROJECT_INDEXES
        .lock()
        .map_err(|_| anyhow!("本地索引管理器锁已损坏"))?;
    if let Some(index) = indexes.get(root) {
        return Ok(Arc::clone(index));
    }

    let config_dir = dirs::config_dir().ok_or_else(|| anyhow!("无法定位系统配置目录"))?;
    let index_dir = config_dir.join("sanshu").join("sou-index");
    fs::create_dir_all(&index_dir).context("创建 sou 本地索引目录失败")?;
    let db_path = index_dir.join(format!("{}.sqlite3", project_hash(root)));
    let index = Arc::new(ProjectIndex::new(root.to_path_buf(), db_path));
    indexes.insert(root.to_path_buf(), Arc::clone(&index));
    Ok(index)
}

fn refresh_profile(index: &ProjectIndex, excludes: &[String]) {
    let profile = profile_hash(excludes);
    if let Ok(mut current) = index.profile_hash.lock() {
        if *current != profile {
            *current = profile;
            index.dirty.store(true, Ordering::Release);
        }
    }
}

fn schedule_sync(index: Arc<ProjectIndex>, exclude_paths: Vec<String>) {
    if index
        .sync_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // 成功占有同步任务后再清除 dirty；同步期间的新事件会重新置位，不会丢失后续变更。
    index.dirty.store(false, Ordering::Release);
    if index.state.load(Ordering::Acquire) != INDEX_READY {
        index.state.store(INDEX_BUILDING, Ordering::Release);
    }

    tokio::spawn(async move {
        let task_index = Arc::clone(&index);
        let result = tokio::task::spawn_blocking(move || sync_index(&task_index, &exclude_paths))
            .await
            .map_err(|error| anyhow!("本地索引同步任务异常: {}", error))
            .and_then(|value| value);
        finish_sync(&index, result);
    });
}

async fn sync_now(index: Arc<ProjectIndex>, exclude_paths: Vec<String>) -> Result<()> {
    if index
        .sync_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(anyhow!("本地索引正在同步，请稍后重试"));
    }
    index.state.store(INDEX_BUILDING, Ordering::Release);
    let task_index = Arc::clone(&index);
    let result = tokio::task::spawn_blocking(move || sync_index(&task_index, &exclude_paths))
        .await
        .map_err(|error| anyhow!("本地索引同步任务异常: {}", error))
        .and_then(|value| value);
    match result {
        Ok(counts) => {
            finish_sync(&index, Ok(counts));
            Ok(())
        }
        Err(error) => {
            let message = error.to_string();
            finish_sync(&index, Err(error));
            Err(anyhow!(message))
        }
    }
}

fn finish_sync(index: &ProjectIndex, result: Result<(u64, u64)>) {
    match result {
        Ok((files, chunks)) => {
            index.indexed_files.store(files, Ordering::Release);
            index.indexed_chunks.store(chunks, Ordering::Release);
            index.state.store(INDEX_READY, Ordering::Release);
            if let Ok(mut error) = index.last_error.lock() {
                *error = None;
            }
            log::info!(
                "[sou-local] 索引同步完成: project={}, files={}, chunks={}",
                index.root.display(),
                files,
                chunks
            );
        }
        Err(error) => {
            mark_index_error(index, &error.to_string());
            log::warn!(
                "[sou-local] 索引同步失败: project={}, error={}",
                index.root.display(),
                error
            );
        }
    }
    index.sync_running.store(false, Ordering::Release);
}

fn mark_index_error(index: &ProjectIndex, message: &str) {
    index.state.store(INDEX_ERROR, Ordering::Release);
    if let Ok(mut error) = index.last_error.lock() {
        *error = Some(message.to_string());
    }
}

fn inspect_existing_index(db_path: &Path) -> (u8, u64, u64, Option<String>) {
    if !db_path.is_file() {
        return (INDEX_MISSING, 0, 0, None);
    }
    match open_database(db_path).and_then(|connection| index_counts(&connection)) {
        Ok((files, chunks)) => (INDEX_READY, files, chunks, None),
        Err(error) => (INDEX_ERROR, 0, 0, Some(error.to_string())),
    }
}

fn open_database(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .with_context(|| format!("打开本地索引失败: {}", path.display()))?;
    connection.busy_timeout(Duration::from_millis(250))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS files (
             path TEXT PRIMARY KEY,
             modified_ns INTEGER NOT NULL,
             size INTEGER NOT NULL
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
             path UNINDEXED,
             start_line UNINDEXED,
             end_line UNINDEXED,
             search_text,
             content UNINDEXED,
             tokenize='unicode61 remove_diacritics 2'
         );",
    )?;
    Ok(connection)
}

fn sync_index(index: &ProjectIndex, exclude_paths: &[String]) -> Result<(u64, u64)> {
    let mut connection = open_database(&index.db_path)?;
    let mut existing = load_file_metadata(&connection)?;
    let files = collect_project_files(&index.root, exclude_paths);
    let transaction = connection.transaction()?;

    for path in files {
        let relative = relative_path(&index.root, &path)?;
        let metadata = match fs::metadata(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let signature = (modified_ns(&metadata), metadata.len() as i64);
        if existing.remove(&relative) == Some(signature) {
            continue;
        }

        transaction.execute("DELETE FROM chunks WHERE path = ?1", params![relative])?;
        transaction.execute("DELETE FROM files WHERE path = ?1", params![relative])?;
        let Some(content) = read_text_file(&path, metadata.len())? else {
            continue;
        };
        transaction.execute(
            "INSERT INTO files(path, modified_ns, size) VALUES (?1, ?2, ?3)",
            params![relative, signature.0, signature.1],
        )?;
        for (start_line, end_line, excerpt) in chunk_content(&content) {
            let search_text = build_search_text(&relative, &excerpt);
            transaction.execute(
                "INSERT INTO chunks(path, start_line, end_line, search_text, content)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    relative,
                    start_line as i64,
                    end_line as i64,
                    search_text,
                    excerpt
                ],
            )?;
        }
    }

    for stale in existing.keys() {
        transaction.execute("DELETE FROM chunks WHERE path = ?1", params![stale])?;
        transaction.execute("DELETE FROM files WHERE path = ?1", params![stale])?;
    }
    transaction.commit()?;
    index_counts(&connection)
}

fn load_file_metadata(connection: &Connection) -> Result<HashMap<String, (i64, i64)>> {
    let mut statement = connection.prepare("SELECT path, modified_ns, size FROM files")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    })?;
    let mut values = HashMap::new();
    for row in rows {
        let (path, signature) = row?;
        values.insert(path, signature);
    }
    Ok(values)
}

fn index_counts(connection: &Connection) -> Result<(u64, u64)> {
    let files =
        connection.query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, u64>(0))?;
    let chunks = connection.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
        row.get::<_, u64>(0)
    })?;
    Ok((files, chunks))
}

fn query_index(
    db_path: &Path,
    query: &str,
    terms: &[String],
    max_results: usize,
) -> Result<Vec<SearchHit>> {
    let connection = open_database(db_path)?;
    let match_query = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let fetch_limit = max_results.max(1).saturating_mul(5).min(150);
    let mut statement = connection.prepare(
        "SELECT path, start_line, end_line, content,
                bm25(chunks, 0.0, 0.0, 0.0, 1.0, 0.0) AS lexical_score
         FROM chunks
         WHERE chunks MATCH ?1
         ORDER BY lexical_score
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![match_query, fetch_limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? as usize,
            row.get::<_, i64>(2)? as usize,
            row.get::<_, String>(3)?,
            row.get::<_, f64>(4)?,
        ))
    })?;

    let mut hits = Vec::new();
    for row in rows {
        let (path, start_line, end_line, excerpt, lexical_score) = row?;
        hits.push(score_hit(
            path,
            start_line,
            end_line,
            excerpt,
            lexical_score,
            query,
            terms,
        ));
    }
    rank_and_limit(hits, max_results)
}

async fn run_immediate_search(
    root: &Path,
    options: &LocalSearchOptions,
    terms: &[String],
) -> Result<(Vec<SearchHit>, String)> {
    match run_rg(
        root,
        &options.query,
        terms,
        options.max_results,
        &options.exclude_paths,
    )
    .await
    {
        Ok(hits) => Ok((hits, "rg".to_string())),
        Err(error) => {
            log::warn!("[sou-local] rg 即时搜索失败，切换 Rust 扫描: {}", error);
            let root = root.to_path_buf();
            let query = options.query.clone();
            let terms = terms.to_vec();
            let excludes = options.exclude_paths.clone();
            let max_results = options.max_results;
            let hits = tokio::task::spawn_blocking(move || {
                scan_project(&root, &query, &terms, max_results, &excludes)
            })
            .await
            .context("等待 Rust 本地扫描任务失败")??;
            Ok((hits, "scan".to_string()))
        }
    }
}

async fn run_rg(
    root: &Path,
    query: &str,
    terms: &[String],
    max_results: usize,
    excludes: &[String],
) -> Result<Vec<SearchHit>> {
    let mut command = Command::new("rg");
    command
        .current_dir(root)
        .kill_on_drop(true)
        .arg("--json")
        .arg("--line-number")
        .arg("--ignore-case")
        .arg("--fixed-strings")
        .arg("--no-messages")
        .arg("--max-count")
        .arg("4")
        .arg("--max-filesize")
        .arg("1M")
        .arg("--glob")
        .arg(code_glob());
    for term in terms.iter().take(12) {
        command.arg("-e").arg(term);
    }
    for exclude in excludes {
        command.arg("--glob").arg(exclude_glob(exclude));
    }
    command
        .arg(".")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .map_err(|_| anyhow!("rg 即时搜索超时"))?
        .context("启动 rg 失败")?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(anyhow!("rg 退出码异常: {:?}", output.status.code()));
    }

    let mut matches: HashMap<String, Vec<usize>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }
        let Some(data) = event.get("data") else {
            continue;
        };
        let Some(path) = data
            .get("path")
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(line_number) = data.get("line_number").and_then(Value::as_u64) else {
            continue;
        };
        let path = normalize_relative(path);
        let entry = matches.entry(path).or_default();
        if entry.len() < 4 {
            entry.push(line_number as usize);
        }
    }

    hits_from_line_matches(root, query, terms, matches, max_results)
}

fn scan_project(
    root: &Path,
    query: &str,
    terms: &[String],
    max_results: usize,
    excludes: &[String],
) -> Result<Vec<SearchHit>> {
    let mut matches = HashMap::new();
    for path in collect_project_files(root, excludes) {
        let metadata = match fs::metadata(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(content) = read_text_file(&path, metadata.len())? else {
            continue;
        };
        let mut lines = Vec::new();
        for (index, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            if terms.iter().any(|term| lower.contains(term)) {
                lines.push(index + 1);
                if lines.len() == 4 {
                    break;
                }
            }
        }
        if !lines.is_empty() {
            matches.insert(relative_path(root, &path)?, lines);
        }
    }
    hits_from_line_matches(root, query, terms, matches, max_results)
}

fn hits_from_line_matches(
    root: &Path,
    query: &str,
    terms: &[String],
    matches: HashMap<String, Vec<usize>>,
    max_results: usize,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    for (path, mut line_numbers) in matches {
        line_numbers.sort_unstable();
        line_numbers.dedup();
        let full_path = root.join(&path);
        let content = fs::read_to_string(&full_path)
            .with_context(|| format!("读取即时搜索命中文件失败: {}", full_path.display()))?;
        let all_lines = content.lines().collect::<Vec<_>>();
        for line_number in line_numbers.into_iter().take(2) {
            let start_line = line_number.saturating_sub(3).max(1);
            let end_line = (line_number + 3).min(all_lines.len());
            let excerpt = all_lines[start_line - 1..end_line].join("\n");
            hits.push(score_hit(
                path.clone(),
                start_line,
                end_line,
                excerpt,
                0.0,
                query,
                terms,
            ));
        }
    }
    rank_and_limit(hits, max_results)
}

fn score_hit(
    relative_path: String,
    start_line: usize,
    end_line: usize,
    excerpt: String,
    lexical_score: f64,
    query: &str,
    terms: &[String],
) -> SearchHit {
    let lower_excerpt = excerpt.to_lowercase();
    let lower_path = relative_path.to_lowercase();
    let coverage = terms
        .iter()
        .filter(|term| lower_excerpt.contains(term.as_str()) || lower_path.contains(term.as_str()))
        .count();
    let path_matches = terms
        .iter()
        .filter(|term| lower_path.contains(term.as_str()))
        .count();
    let normalized_query = query.trim().to_lowercase();
    let exact_match = !normalized_query.is_empty()
        && (lower_excerpt.contains(&normalized_query) || lower_path.contains(&normalized_query));
    SearchHit {
        relative_path,
        start_line,
        end_line,
        excerpt,
        coverage,
        exact_match,
        path_matches,
        lexical_score,
    }
}

fn rank_and_limit(mut hits: Vec<SearchHit>, max_results: usize) -> Result<Vec<SearchHit>> {
    hits.sort_by(|left, right| {
        right
            .coverage
            .cmp(&left.coverage)
            .then_with(|| right.exact_match.cmp(&left.exact_match))
            .then_with(|| right.path_matches.cmp(&left.path_matches))
            .then_with(|| {
                left.lexical_score
                    .partial_cmp(&right.lexical_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.relative_path.cmp(&right.relative_path))
            .then_with(|| left.start_line.cmp(&right.start_line))
    });
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert((hit.relative_path.clone(), hit.start_line, hit.end_line)));
    hits.truncate(max_results.max(1));
    Ok(hits)
}

fn format_hits(
    root: &Path,
    hits: &[SearchHit],
    engine: &str,
    state: &str,
    duration_ms: u64,
    fallback_reason: Option<&str>,
) -> String {
    let mut parts = vec![
        "The following code sections were retrieved:".to_string(),
        String::new(),
    ];
    for hit in hits {
        parts.push(format!(
            "Path: {}",
            normalize_path(&root.join(&hit.relative_path))
        ));
        parts.push(format!("Lines: L{}-L{}", hit.start_line, hit.end_line));
        for (offset, line) in hit.excerpt.lines().enumerate() {
            parts.push(format!("L{}:{}", hit.start_line + offset, line));
        }
        parts.push(String::new());
    }
    if hits.is_empty() {
        parts.push("No relevant files found.".to_string());
    }
    parts.push(format!(
        "[sou-local] engine={}, index_state={}, hits={}, duration_ms={}",
        engine,
        state,
        hits.len(),
        duration_ms
    ));
    if let Some(reason) = fallback_reason {
        parts.push(format!("[sou-local fallback] {}", reason));
    }
    parts.join("\n")
}

fn collect_project_files(root: &Path, excludes: &[String]) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| is_supported_file(path))
        .filter(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.len() <= MAX_FILE_BYTES)
                .unwrap_or(false)
        })
        .filter(|path| !is_excluded(root, path, excludes))
        .collect()
}

fn is_supported_file(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if extension.as_deref().is_some_and(|value| {
        matches!(
            value,
            "rs" | "c"
                | "cc"
                | "cpp"
                | "cxx"
                | "h"
                | "hh"
                | "hpp"
                | "cs"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "swift"
                | "scala"
                | "py"
                | "rb"
                | "php"
                | "lua"
                | "js"
                | "mjs"
                | "cjs"
                | "ts"
                | "tsx"
                | "jsx"
                | "vue"
                | "svelte"
                | "astro"
                | "html"
                | "css"
                | "scss"
                | "sass"
                | "less"
                | "sql"
                | "graphql"
                | "gql"
                | "proto"
                | "xml"
                | "json"
                | "jsonc"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "md"
                | "mdx"
                | "txt"
                | "rst"
                | "adoc"
                | "sh"
                | "bash"
                | "zsh"
                | "fish"
                | "ps1"
                | "bat"
        )
    }) {
        return true;
    }
    matches!(
        path.file_name().and_then(|value| value.to_str()),
        Some("Dockerfile" | "Makefile" | "CMakeLists.txt" | "Justfile")
    )
}

fn is_excluded(root: &Path, path: &Path, excludes: &[String]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let normalized = normalize_path(relative);
    excludes.iter().any(|exclude| {
        let exclude = exclude
            .trim()
            .trim_start_matches("./")
            .trim_matches('/')
            .replace('\\', "/");
        if exclude.is_empty() {
            return false;
        }
        let plain = exclude.trim_matches('*').trim_matches('/');
        normalized == plain
            || normalized.starts_with(&format!("{}/", plain))
            || normalized.contains(&format!("/{}/", plain))
            || normalized.ends_with(&format!("/{}", plain))
    })
}

fn read_text_file(path: &Path, size: u64) -> Result<Option<String>> {
    if size > MAX_FILE_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(path).with_context(|| format!("读取源码失败: {}", path.display()))?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn chunk_content(content: &str) -> Vec<(usize, usize, String)> {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let step = CHUNK_LINES - CHUNK_OVERLAP;
    let mut start = 0usize;
    while start < lines.len() {
        let end = (start + CHUNK_LINES).min(lines.len());
        chunks.push((start + 1, end, lines[start..end].join("\n")));
        if end == lines.len() {
            break;
        }
        start += step;
    }
    chunks
}

fn build_search_text(path: &str, content: &str) -> String {
    tokenize_text(&format!("{}\n{}", path, content), usize::MAX).join(" ")
}

fn extract_query_terms(query: &str) -> Vec<String> {
    let stopwords = [
        "the", "and", "for", "from", "with", "this", "that", "what", "where", "when", "代码",
        "项目", "搜索", "相关", "实现", "如何", "怎么", "什么", "是否",
    ];
    let mut terms = tokenize_text(query, usize::MAX)
        .into_iter()
        .filter(|term| term.len() >= 2 && !stopwords.contains(&term.as_str()))
        .collect::<Vec<_>>();
    // 混合长句优先保留代码标识符，其次保留中文二/三元词，避免自然语言前缀挤掉后置函数名。
    terms.sort_by_key(|term| {
        if term.is_ascii() {
            0
        } else if (2..=3).contains(&term.chars().count()) {
            1
        } else {
            2
        }
    });
    terms.truncate(MAX_QUERY_TERMS);
    terms
}

fn tokenize_text(text: &str, limit: usize) -> Vec<String> {
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    while index < chars.len() && output.len() < limit {
        if chars[index].is_ascii_alphanumeric()
            || matches!(chars[index], '_' | '-' | '.' | '/' | ':' | '\\')
        {
            let start = index;
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric()
                    || matches!(chars[index], '_' | '-' | '.' | '/' | ':' | '\\'))
            {
                index += 1;
            }
            let raw = chars[start..index].iter().collect::<String>();
            push_ascii_tokens(&raw, &mut output, &mut seen, limit);
            continue;
        }
        if is_cjk(chars[index]) {
            let start = index;
            index += 1;
            while index < chars.len() && is_cjk(chars[index]) {
                index += 1;
            }
            let run = chars[start..index].iter().collect::<String>();
            push_token(&run, &mut output, &mut seen, limit);
            let run_chars = run.chars().collect::<Vec<_>>();
            for pair in run_chars.windows(2) {
                push_token(
                    &pair.iter().collect::<String>(),
                    &mut output,
                    &mut seen,
                    limit,
                );
            }
            for triple in run_chars.windows(3) {
                push_token(
                    &triple.iter().collect::<String>(),
                    &mut output,
                    &mut seen,
                    limit,
                );
            }
            continue;
        }
        index += 1;
    }
    output
}

fn push_ascii_tokens(
    raw: &str,
    output: &mut Vec<String>,
    seen: &mut HashSet<String>,
    limit: usize,
) {
    let trimmed = raw.trim_matches(|ch: char| matches!(ch, '.' | '/' | ':' | '\\' | '-' | '_'));
    if trimmed.is_empty() {
        return;
    }
    push_token(&trimmed.to_ascii_lowercase(), output, seen, limit);
    for segment in trimmed.split(['_', '-', '.', '/', ':', '\\']) {
        if segment.is_empty() {
            continue;
        }
        push_token(&segment.to_ascii_lowercase(), output, seen, limit);
        for part in split_camel_case(segment) {
            push_token(&part.to_ascii_lowercase(), output, seen, limit);
        }
    }
}

fn split_camel_case(value: &str) -> Vec<String> {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() < 2 {
        return vec![value.to_string()];
    }
    let mut parts = Vec::new();
    let mut start = 0usize;
    for index in 1..chars.len() {
        let previous = chars[index - 1];
        let current = chars[index];
        let next = chars.get(index + 1).copied();
        let boundary = (previous.is_ascii_lowercase() || previous.is_ascii_digit())
            && current.is_ascii_uppercase()
            || previous.is_ascii_uppercase()
                && current.is_ascii_uppercase()
                && next.is_some_and(|value| value.is_ascii_lowercase());
        if boundary {
            parts.push(chars[start..index].iter().collect());
            start = index;
        }
    }
    parts.push(chars[start..].iter().collect());
    parts
}

fn push_token(value: &str, output: &mut Vec<String>, seen: &mut HashSet<String>, limit: usize) {
    let value = value.trim().to_lowercase();
    if value.is_empty() || output.len() >= limit || !seen.insert(value.clone()) {
        return;
    }
    output.push(value);
}

fn is_cjk(value: char) -> bool {
    matches!(value as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
}

fn modified_ns(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    Ok(normalize_path(path.strip_prefix(root).with_context(
        || format!("文件不在项目目录内: {}", path.display()),
    )?))
}

fn normalize_relative(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn project_hash(root: &Path) -> String {
    let mut normalized = normalize_path(root);
    if cfg!(windows) {
        normalized = normalized.to_ascii_lowercase();
    }
    let mut context = ShaContext::new(&SHA256);
    context.update(normalized.as_bytes());
    hex::encode(&context.finish().as_ref()[..16])
}

fn profile_hash(excludes: &[String]) -> String {
    let mut values = excludes
        .iter()
        .map(|value| value.trim().replace('\\', "/"))
        .collect::<Vec<_>>();
    values.sort();
    let mut context = ShaContext::new(&SHA256);
    context.update(values.join("\n").as_bytes());
    hex::encode(&context.finish().as_ref()[..8])
}

fn code_glob() -> &'static str {
    "*.{rs,c,cc,cpp,cxx,h,hh,hpp,cs,go,java,kt,kts,swift,scala,py,rb,php,lua,js,mjs,cjs,ts,tsx,jsx,vue,svelte,astro,html,css,scss,sass,less,sql,graphql,gql,proto,xml,json,jsonc,yaml,yml,toml,ini,md,mdx,txt,rst,adoc,sh,bash,zsh,fish,ps1,bat}"
}

fn exclude_glob(value: &str) -> String {
    let normalized = value
        .trim()
        .trim_start_matches("./")
        .trim_matches('/')
        .replace('\\', "/");
    if normalized.starts_with('!') {
        normalized
    } else if normalized.contains('*') || normalized.contains('/') {
        format!("!{}", normalized)
    } else {
        format!("!**/{}/**", normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn query_terms_cover_identifiers_and_chinese_bigrams() {
        let terms = extract_query_terms("ScopeWorkspace scope_name 手机号验证码");
        assert!(terms.contains(&"scopeworkspace".to_string()));
        assert!(terms.contains(&"scope".to_string()));
        assert!(terms.contains(&"workspace".to_string()));
        assert!(terms.contains(&"手机".to_string()));
        assert!(terms.contains(&"验证码".to_string()));
    }

    #[test]
    fn long_chinese_query_keeps_trailing_code_identifier() {
        let terms = extract_query_terms(
            "请在整个大型项目中查找手机号验证码登录实现与权限路由链路 ScopeWorkspace",
        );
        assert!(terms.contains(&"scopeworkspace".to_string()));
    }

    #[test]
    fn fts5_index_supports_warm_multi_keyword_search_and_incremental_update() {
        let temp = tempdir().expect("临时项目应创建成功");
        let root = temp.path().join("project");
        fs::create_dir_all(root.join("src")).expect("源码目录应创建成功");
        let source = root.join("src").join("scope_workspace.rs");
        fs::write(
            &source,
            "pub struct ScopeWorkspace;\nfn append_current_options(scope_name: &str) {}\n",
        )
        .expect("测试源码应写入成功");
        let index = ProjectIndex::new(root.clone(), temp.path().join("index.sqlite3"));

        let first_counts = sync_index(&index, &[]).expect("首次索引应成功");
        assert_eq!(first_counts.0, 1);
        let terms = extract_query_terms("ScopeWorkspace scopeName appendCurrentOptions");
        let hits =
            query_index(&index.db_path, "ScopeWorkspace", &terms, 10).expect("热索引查询应成功");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].relative_path, "src/scope_workspace.rs");

        std::thread::sleep(Duration::from_millis(2));
        fs::write(&source, "pub struct RenamedWorkspace;\n").expect("测试源码应更新成功");
        sync_index(&index, &[]).expect("增量索引应成功");
        let renamed = query_index(
            &index.db_path,
            "RenamedWorkspace",
            &extract_query_terms("RenamedWorkspace"),
            10,
        )
        .expect("更新后的索引查询应成功");
        assert_eq!(renamed.len(), 1);
    }

    #[tokio::test]
    async fn pending_index_changes_use_current_files_instead_of_stale_fts5() {
        let temp = tempdir().expect("即时搜索测试目录应创建成功");
        let root = temp.path().join("project");
        fs::create_dir_all(&root).expect("即时搜索测试项目应创建成功");
        let source = root.join("search_state.rs");
        fs::write(&source, "pub struct OldIndexValue;\n").expect("旧索引源码应写入成功");
        let index = Arc::new(ProjectIndex::new(
            root.clone(),
            temp.path().join("pending.sqlite3"),
        ));
        sync_now(Arc::clone(&index), Vec::new())
            .await
            .expect("旧内容索引应建立成功");

        fs::write(&source, "pub struct CurrentFileValue;\n").expect("当前源码应写入成功");
        index.dirty.store(true, Ordering::Release);
        // 模拟已有增量同步任务，避免测试结束后遗留后台任务。
        index.sync_running.store(true, Ordering::Release);
        let output = search_with_index(
            LocalSearchOptions {
                project_root: root.clone(),
                query: "CurrentFileValue".to_string(),
                max_results: 5,
                exclude_paths: Vec::new(),
            },
            root,
            Arc::clone(&index),
            false,
        )
        .await
        .expect("待同步状态应使用即时搜索");
        index.sync_running.store(false, Ordering::Release);

        assert_ne!(output.engine, "fts5");
        assert_eq!(output.hit_count, 1);
        assert!(output.text.contains("CurrentFileValue"));
    }

    #[test]
    #[ignore = "由 scripts/test-sou-local-fallback.ps1 显式执行性能基准"]
    fn warm_fts5_query_p95_is_within_target_for_thousands_of_files() {
        let temp = tempdir().expect("性能测试目录应创建成功");
        let root = temp.path().join("project");
        fs::create_dir_all(root.join("src")).expect("性能测试源码目录应创建成功");
        for index in 0..2500 {
            fs::write(
                root.join("src").join(format!("module_{index}.rs")),
                format!(
                    "pub struct ScopeWorkspace{index};\nfn append_current_options_{index}(scope_name: &str) {{}}\n"
                ),
            )
            .expect("性能测试源码应写入成功");
        }
        let index = ProjectIndex::new(root, temp.path().join("bench.sqlite3"));
        sync_index(&index, &[]).expect("性能测试索引应建立成功");
        let query = "ScopeWorkspace appendCurrentOptions scopeName";
        let terms = extract_query_terms(query);

        let mut durations = Vec::new();
        for _ in 0..80 {
            let started = Instant::now();
            let hits =
                query_index(&index.db_path, query, &terms, 10).expect("性能测试热查询应成功");
            assert_eq!(hits.len(), 10);
            durations.push(started.elapsed().as_micros() as u64);
        }
        durations.sort_unstable();
        let p95_us = durations[durations.len() * 95 / 100];
        println!(
            "sou_local_benchmark files=2500 queries=80 p95_us={} target_us=50000",
            p95_us
        );
        assert!(
            p95_us <= 50_000,
            "warm FTS5 p95 超过 50ms 目标: {}us",
            p95_us
        );
    }
}
