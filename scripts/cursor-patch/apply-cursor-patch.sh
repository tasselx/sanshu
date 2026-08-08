#!/usr/bin/env bash
# Cursor workbench patch：断流自动重试时不铸新 attempt requestId，
# 并强制出网 x-request-id = x-original-request-id（双保险）。
# 说明见同目录 README.md
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BACKUP_ROOT="$SCRIPT_DIR/backups"
ARTIFACTS_DIR="$SCRIPT_DIR/artifacts"
SLOW_POOL_DUMP_PATH="${SLOW_POOL_DUMP_PATH:-$ARTIFACTS_DIR/slow-pool-models.txt}"
MARKER_TAG="sanshu-cursor-patch"
PATCH_VER="v2"

WB_DIR="/Applications/Cursor.app/Contents/Resources/app/out/vs/workbench"
DESKTOP="$WB_DIR/workbench.desktop.main.js"
GLASS="$WB_DIR/workbench.glass.main.js"
CURSOR_APP="/Applications/Cursor.app"

die() { echo "错误: $*" >&2; exit 1; }
# 进度信息走 stderr，避免被 $(backup_once) 之类捕获污染
info() { echo "$*" >&2; }
warn() { echo "警告: $*" >&2; }

require_python() {
  command -v python3 >/dev/null 2>&1 || die "需要 python3"
}

require_files() {
  [[ -f "$DESKTOP" ]] || die "找不到 $DESKTOP（Cursor 是否安装在 $CURSOR_APP？）"
  [[ -f "$GLASS" ]] || die "找不到 $GLASS"
}

require_writable() {
  require_files
  [[ -w "$DESKTOP" ]] || die "无写权限: $DESKTOP（可试: sudo chown \"\$USER\" ...）"
  [[ -w "$GLASS" ]] || die "无写权限: $GLASS"
}

sha() { shasum -a 256 "$1" | awk '{print $1}'; }

cursor_version() {
  if [[ -f "$CURSOR_APP/Contents/Info.plist" ]]; then
    /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
      "$CURSOR_APP/Contents/Info.plist" 2>/dev/null && return
  fi
  echo "未知"
}

latest_backup_dir() {
  ls -1dt "$BACKUP_ROOT"/*/ 2>/dev/null | head -1 || true
}

backup_meta_version() {
  local dir="$1"
  [[ -f "$dir/meta.txt" ]] || { echo ""; return; }
  sed -n 's/^cursor_version=//p' "$dir/meta.txt" | head -1
}

# 备份当前 live bundle；打印备份路径到 stdout 最后一行之前用 info
backup_once() {
  local ts dest note="${1:-}"
  ts="$(date +%Y%m%dT%H%M%S)"
  dest="$BACKUP_ROOT/$ts"
  mkdir -p "$dest"
  cp -p "$DESKTOP" "$dest/workbench.desktop.main.js"
  cp -p "$GLASS" "$dest/workbench.glass.main.js"
  {
    echo "timestamp=$ts"
    echo "desktop_sha=$(sha "$DESKTOP")"
    echo "glass_sha=$(sha "$GLASS")"
    echo "cursor_version=$(cursor_version)"
    echo "patch_ver=$PATCH_VER"
    [[ -n "$note" ]] && echo "note=$note"
  } >"$dest/meta.txt"
  info "已备份当前文件 -> $dest"
  # 仅路径走 stdout，供调用方捕获
  printf '%s\n' "$dest"
}

# ---------------------------------------------------------------------------
# Python 核心：探测 / 变换 / 校验（全部逻辑在此，bash 只做 IO 与备份）
# 环境变量:
#   PROBE_CMD = status|check|dry-run|apply-reuse|apply-disable|selftest
#   PROBE_MODE, MARKER_TAG, PATCH_VER, DESKTOP, GLASS, CURSOR_VER
#   FIXTURE_DIR (selftest), WRITE=1 (apply 才写盘)
# ---------------------------------------------------------------------------
run_core() {
  require_python
  local cmd="$1"
  shift || true
  MARKER_TAG="$MARKER_TAG" \
  PATCH_VER="$PATCH_VER" \
  DESKTOP="${DESKTOP:-}" \
  GLASS="${GLASS:-}" \
  CURSOR_VER="$(cursor_version)" \
  PROBE_CMD="$cmd" \
  FIXTURE_DIR="${FIXTURE_DIR:-}" \
  WRITE="${WRITE:-0}" \
  BACKUP_DIR="${BACKUP_DIR:-}" \
  SLOW_POOL_DUMP_PATH="$SLOW_POOL_DUMP_PATH" \
  WITH_SLOW_DUMP="${WITH_SLOW_DUMP:-0}" \
  python3 - "$@" <<'PY'
import hashlib
import json
import os
import re
import sys
import tempfile
from pathlib import Path

tag = os.environ["MARKER_TAG"]
pver = os.environ.get("PATCH_VER") or "v2"
cmd = os.environ["PROBE_CMD"]
ver = os.environ.get("CURSOR_VER") or "未知"
write = os.environ.get("WRITE") == "1"
backup_dir = os.environ.get("BACKUP_DIR") or ""
slow_pool_dump_path = os.environ.get("SLOW_POOL_DUMP_PATH") or ""
with_slow_dump = os.environ.get("WITH_SLOW_DUMP", "0") == "1"

# ---- patterns (tolerant to minified identifier names) ----

# Stock: attempt>0?(VAR=crypto.randomUUID(),await X.onRetryStarting?.(
RE_STOCK_MINT = re.compile(
    r"([A-Za-z_$][\w$]*>0\?\()"
    r"([A-Za-z_$][\w$]*=crypto\.randomUUID\(\),)"
    r"(await [A-Za-z_$][\w$]*\.onRetryStarting\?\.\()"
)
# Patched mint: attempt>0?(await X.onRetryStarting?.(
RE_PATCHED_MINT = re.compile(
    r"[A-Za-z_$][\w$]*>0\?\(await [A-Za-z_$][\w$]*\.onRetryStarting\?\.\("
)
# Stock header object (attempt id != original id)
RE_STOCK_HDR = re.compile(
    r'(\{"x-request-id":)([A-Za-z_$][\w$]*)(,"x-original-request-id":)([A-Za-z_$][\w$]*)'
)
# Patched header: both sides same identifier
RE_PATCHED_HDR = re.compile(
    r'\{"x-request-id":([A-Za-z_$][\w$]*),"x-original-request-id":\1\b'
)

GATE_STOCK = 'this.experimentService.checkFeatureGate("nal_agent_retries",{disableExposureLog:!1})'
DEFAULT_ON = "nal_agent_retries:{client:!0,default:!0}"
DEFAULT_OFF = "nal_agent_retries:{client:!0,default:!1}"

MARKER_REUSE = f"/*{tag}:reuse*/"
MARKER_REUSE_V2 = f"/*{tag}:reuse:{pver}*/"
MARKER_DISABLE = f"/*{tag}:disable*/"
MARKER_SLOW_DUMP = f"/*{tag}:slow-dump*/"
MARKER_SLOW_DUMP_ERR = f"/*{tag}:slow-dump-err*/"

# usageLimitPolicyStatusService.performFetch: after setStatusData + setActiveGrantsData
RE_STOCK_SLOW_DUMP = re.compile(
    r"this\.setStatusData\(([A-Za-z_$][\w$]*)\),"
    r"([A-Za-z_$][\w$]*)\(\1\)\|\|this\.setDismissedAutoSwitchStatusFingerprintData\(null\);"
    r"const ([A-Za-z_$][\w$]*)=([A-Za-z_$][\w$]*)\.activeGrants\.map\(.*?\)\);"
    r"this\.setActiveGrantsData\(\3\)"
    rf"(?!;\s*{re.escape(MARKER_SLOW_DUMP)})",
    re.S,
)
RE_PATCHED_SLOW_DUMP = re.compile(re.escape(MARKER_SLOW_DUMP))

# Expected-ish counts on known builds (warning only)
EXPECT_MINT_PER_FILE = 2
EXPECT_HDR_PER_FILE = 1
EXPECT_SLOW_DUMP_PER_FILE = 1


# Existing dumped IIFE (marker + (function...)(A,B)) for replace/upgrade
RE_EXISTING_SLOW_DUMP = re.compile(
    re.escape(MARKER_SLOW_DUMP)
    + r"\(function\(S,G\)\{.*?\}\)"
    + r"\(([A-Za-z_$][\w$]*),([A-Za-z_$][\w$]*)\)",
    re.S,
)

RE_CATCH_SLOW_DUMP = re.compile(
    r'console\.error\("Failed to fetch usage limit status and active grants:",([A-Za-z_$][\w$]*)\),'
    r'this\.setError\(\1 instanceof Error\?\1\.message:"Failed to fetch usage limit status and active grants"\),'
    r'this\.setStatusData\(null\),this\.setActiveGrantsData\(\[\]\),\{success:!1\}'
    rf'(?!;\s*{re.escape(MARKER_SLOW_DUMP_ERR)})'
)


def build_slow_dump_iife() -> str:
    """Compact IIFE: dump status+grants as plain text; multi-path write + download fallback."""
    fp = json.dumps(slow_pool_dump_path)
    # Keep single-expression friendly; no template literals.
    return (
        "(function(S,G){try{"
        "var ids=(S&&S.allowedModelIds)||[],tags=(S&&S.allowedModelTags)||[],gs=G||[],"
        "iso=(new Date).toISOString(),"
        "primary=" + fp + ","
        "home=(typeof process!==\"undefined\"&&process.env&&(process.env.HOME||process.env.USERPROFILE))||\"\","
        "paths=[primary];"
        "if(home){paths.push(home+\"/Library/Application Support/Cursor/sanshu-slow-pool-models.txt\");"
        "paths.push(home+\"/Downloads/sanshu-slow-pool-models.txt\");}"
        "paths.push(\"/tmp/sanshu-slow-pool-models.txt\");"
        "function jn(a){return(a&&a.length)?a.join(\", \"):\"(none)\"}"
        "var lines=["
        "\"# sanshu slow-pool dump\","
        "\"path_primary: \"+primary,"
        "\"iso: \"+iso,"
        "\"ts: \"+Date.now(),"
        "\"isInSlowPool: \"+!!(S&&S.isInSlowPool),"
        "\"stage: \"+String(S&&S.stage),"
        "\"slownessMs: \"+String(S&&S.slownessMs),"
        "\"limitType: \"+String(S&&S.limitType),"
        "\"trayLabel: \"+String((S&&S.trayLabel)||\"\"),"
        "\"ctaLabel: \"+String((S&&S.ctaLabel)||\"\"),"
        "\"errorTitle: \"+String((S&&S.errorTitle)||\"\"),"
        "\"errorDetail: \"+String((S&&S.errorDetail)||\"\"),"
        "\"allowedModelIds: \"+jn(ids),"
        "\"allowedModelTags: \"+jn(tags),"
        "\"grants:\""
        "];"
        "for(var i=0;i<gs.length;i++){var g=gs[i]||{};"
        "lines.push(\"- id=\"+String(g.grantId)"
        "+\" remainingCents=\"+String(g.remainingCents)"
        "+\" slownessMs=\"+String(g.slownessMs)"
        "+\" ids=\"+jn(g.allowedModelIds||[])"
        "+\" tags=\"+jn(g.allowedModelTags||[])"
        "+\" source=\"+String(g.source||\"\")"
        "+\" type=\"+String(g.grantType||\"\"));}"
        "if(!gs.length)lines.push(\"(none)\");"
        "var txt=lines.join(\"\\n\")+\"\\n\";"
        "var meta={path:primary,iso:iso,isInSlowPool:!!(S&&S.isInSlowPool),stage:S&&S.stage,"
        "slownessMs:S&&S.slownessMs,allowedModelIds:ids,allowedModelTags:tags,text:txt,written:[]};"
        "globalThis.__sanshuSlowPool=meta;"
        "try{localStorage.setItem(\"sanshu-slow-pool-models\",txt)}catch(e0){}"
        "function __fs(){var fs,pm;"
        "try{if(typeof process!==\"undefined\"&&typeof process.getBuiltinModule===\"function\")"
        "{fs=process.getBuiltinModule(\"fs\");pm=process.getBuiltinModule(\"path\")}}catch(e1){}"
        "try{if(!fs&&typeof require===\"function\"){fs=require(\"fs\");pm=require(\"path\")}}catch(e2){}"
        "return{fs:fs,pm:pm}}"
        "function __wOne(fp,t){var m=__fs(),fs=m.fs,pm=m.pm;if(!fs||!fs.writeFileSync)return!1;"
        "try{var d=pm&&pm.dirname?pm.dirname(fp):(function(x){var k=String(x).lastIndexOf(\"/\");"
        "return k>0?String(x).slice(0,k):\".\"})(fp);"
        "fs.mkdirSync&&fs.mkdirSync(d,{recursive:!0});fs.writeFileSync(fp,t,\"utf8\");return!0}"
        "catch(e4){return!1}}"
        "for(var pi=0;pi<paths.length;pi++){if(__wOne(paths[pi],txt))meta.written.push(paths[pi])}"
        "if(!meta.written.length){try{"
        "var blob=new Blob([txt],{type:\"text/plain\"}),url=(URL||webkitURL).createObjectURL(blob),"
        "a=document.createElement(\"a\");a.href=url;a.download=\"sanshu-slow-pool-models.txt\";"
        "a.style.display=\"none\";(document.body||document.documentElement).appendChild(a);a.click();"
        "setTimeout(function(){try{a.remove();(URL||webkitURL).revokeObjectURL(url)}catch(e5){}},2e3);"
        "meta.written.push(\"(browser-download:Downloads/sanshu-slow-pool-models.txt)\")"
        "}catch(e6){}}"
        "try{console.error(\"[sanshu-slow-pool] path=\"+primary+\"; written=\"+JSON.stringify(meta.written)+\"; isInSlowPool=\"+meta.isInSlowPool+\"; stage=\"+String(meta.stage)+\"; ids=\"+jn(ids)+\"; tags=\"+jn(tags))}catch(e7){}"
        "}catch(e){try{console.error(\"[sanshu-slow-pool] err\",e)}catch(e8){}}})"
    )




def load(p: Path) -> str:
    return p.read_text(encoding="utf-8", errors="strict")


def sha16(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()[:16]


def stock_hdr_matches(text: str):
    """Only count headers where attempt var != original var."""
    out = []
    for m in RE_STOCK_HDR.finditer(text):
        if m.group(2) != m.group(4):
            out.append(m)
    return out


def analyze(text: str) -> dict:
    stock_mint = list(RE_STOCK_MINT.finditer(text))
    patched_mint = list(RE_PATCHED_MINT.finditer(text))
    stock_hdr = stock_hdr_matches(text)
    patched_hdr = list(RE_PATCHED_HDR.finditer(text))
    stock_slow = list(RE_STOCK_SLOW_DUMP.finditer(text))
    patched_slow = len(RE_PATCHED_SLOW_DUMP.findall(text)) + (1 if MARKER_SLOW_DUMP_ERR in text else 0)
    return {
        "marker_reuse": MARKER_REUSE in text or MARKER_REUSE_V2 in text,
        "marker_reuse_v2": MARKER_REUSE_V2 in text,
        "marker_disable": MARKER_DISABLE in text,
        "marker_slow_dump": MARKER_SLOW_DUMP in text,
        "stock_mint": len(stock_mint),
        "patched_mint": len(patched_mint),
        "stock_hdr": len(stock_hdr),
        "patched_hdr": len(patched_hdr),
        "stock_slow_dump": len(stock_slow),
        "patched_slow_dump": patched_slow,
        "gate_stock": text.count(GATE_STOCK),
        "default_on": text.count(DEFAULT_ON),
        "default_off": text.count(DEFAULT_OFF),
        "_stock_mint_ms": stock_mint,
        "_stock_hdr_ms": stock_hdr,
        "_stock_slow_ms": stock_slow,
    }


def insert_marker(text: str, marker: str) -> str:
    if marker in text:
        return text
    for anchor in (DEFAULT_ON, DEFAULT_OFF, "[nal_agent_retries]", "nal_agent_retries"):
        if anchor in text:
            return text.replace(anchor, marker + anchor, 1)
    return marker + text


def apply_reuse_layers(text: str) -> tuple[str, dict]:
    """
    Dual insurance:
      1) strip retry crypto.randomUUID() mint
      2) force header x-request-id = x-original-request-id
    Idempotent: already-patched sites are left alone.
    """
    stats = {"mint": 0, "hdr": 0}

    def repl_mint(m: re.Match) -> str:
        stats["mint"] += 1
        return m.group(1) + m.group(3)

    new = RE_STOCK_MINT.sub(repl_mint, text)

    def repl_hdr(m: re.Match) -> str:
        attempt, original = m.group(2), m.group(4)
        if attempt == original:
            return m.group(0)
        stats["hdr"] += 1
        # {"x-request-id":ORIG,"x-original-request-id":ORIG
        return f"{m.group(1)}{original}{m.group(3)}{original}"

    new = RE_STOCK_HDR.sub(repl_hdr, new)

    changed = stats["mint"] + stats["hdr"]
    if changed:
        # Prefer v2 marker; keep old reuse marker compatible
        if MARKER_REUSE_V2 not in new:
            new = insert_marker(new, MARKER_REUSE_V2)
        if MARKER_REUSE not in new:
            new = insert_marker(new, MARKER_REUSE)
    return new, stats


def apply_disable(text: str) -> tuple[str, dict]:
    stats = {"gate": 0, "default": 0}
    if MARKER_DISABLE in text and text.count(GATE_STOCK) == 0:
        return text, stats
    n = text.count(GATE_STOCK)
    if n:
        text = text.replace(GATE_STOCK, "(!1)")
        stats["gate"] = n
    n2 = text.count(DEFAULT_ON)
    if n2:
        text = text.replace(DEFAULT_ON, DEFAULT_OFF)
        stats["default"] = n2
    if stats["gate"] + stats["default"]:
        text = insert_marker(text, MARKER_DISABLE)
    return text, stats


def apply_slow_dump_layer(text: str) -> tuple[str, dict]:
    """Hook usageLimitPolicyStatusService.performFetch → dump slow-pool allowlist as txt."""
    stats = {"slow_dump": 0, "slow_dump_err": 0}
    if not slow_pool_dump_path:
        return text, stats
    iife = build_slow_dump_iife()
    # Upgrade existing inject (path/format changes) while keeping arg names
    m_ex = RE_EXISTING_SLOW_DUMP.search(text)
    if m_ex:
        desired = f"{MARKER_SLOW_DUMP}{iife}({m_ex.group(1)},{m_ex.group(2)})"
        if m_ex.group(0) != desired:
            text = text[: m_ex.start()] + desired + text[m_ex.end() :]
            stats["slow_dump"] = 1
    else:
        m = RE_STOCK_SLOW_DUMP.search(text)
        if m:
            status_v, grants_v = m.group(1), m.group(3)
            injection = f"{m.group(0)};{MARKER_SLOW_DUMP}{iife}({status_v},{grants_v})"
            text = text[: m.start()] + injection + text[m.end() :]
            stats["slow_dump"] = 1

    # Dump on fetch failure so "no file" is diagnosable
    if MARKER_SLOW_DUMP_ERR not in text:
        m2 = RE_CATCH_SLOW_DUMP.search(text)
        if m2:
            err_v = m2.group(1)
            det = (
                f'{err_v} instanceof Error ? {err_v}.message : String({err_v})'
            )
            dump_call = (
                f"{MARKER_SLOW_DUMP_ERR}{iife}"
                f"({{isInSlowPool:!1,stage:\"fetch_error\",errorTitle:\"fetch_failed\","
                f"errorDetail:({det})}},[])"
            )
            # Original: console.error(...,E),this.setError(...),this.setStatusData(null),this.setActiveGrantsData([]),{success:!1}
            # Insert dump after setError,
            rep = (
                f'console.error("Failed to fetch usage limit status and active grants:",{err_v}),'
                f'this.setError({err_v} instanceof Error?{err_v}.message:"Failed to fetch usage limit status and active grants"),'
                f'{dump_call},'
                f'this.setStatusData(null),this.setActiveGrantsData([]),{{success:!1}}'
            )
            text = text[: m2.start()] + rep + text[m2.end() :]
            stats["slow_dump_err"] = 1
            stats["slow_dump"] = stats["slow_dump"] or 1
    return text, stats


def slow_dump_effective(a: dict) -> bool:
    return a["patched_slow_dump"] > 0 and a["stock_slow_dump"] == 0


def slow_dump_applicable(a: dict) -> bool:
    return a["stock_slow_dump"] > 0


def slow_dump_needs_upgrade(text: str) -> bool:
    """True if inject missing or path/format differs from desired."""
    if not slow_pool_dump_path or not with_slow_dump:
        return False
    need = False
    m_ex = RE_EXISTING_SLOW_DUMP.search(text)
    if m_ex:
        desired = f"{MARKER_SLOW_DUMP}{build_slow_dump_iife()}({m_ex.group(1)},{m_ex.group(2)})"
        if m_ex.group(0) != desired:
            need = True
    elif RE_STOCK_SLOW_DUMP.search(text):
        need = True
    # catch path also required when present in stock
    if RE_CATCH_SLOW_DUMP.search(text) and MARKER_SLOW_DUMP_ERR not in text:
        need = True
    # old path alone but primary path not in inject yet
    if MARKER_SLOW_DUMP in text and slow_pool_dump_path and slow_pool_dump_path not in text:
        need = True
    return need


def reuse_effective(a: dict) -> bool:
    """reuse is effective when mint stripped AND header forced (or no stock left)."""
    mint_ok = a["stock_mint"] == 0 and a["patched_mint"] > 0
    hdr_ok = a["stock_hdr"] == 0 and a["patched_hdr"] > 0
    # If header pattern vanished entirely after upgrade, still accept mint-only as partial
    return mint_ok and hdr_ok


def reuse_partial(a: dict) -> bool:
    mint_ok = a["stock_mint"] == 0 and a["patched_mint"] > 0
    hdr_ok = a["stock_hdr"] == 0 and a["patched_hdr"] > 0
    return (mint_ok or hdr_ok) and not (mint_ok and hdr_ok)


def reuse_applicable(a: dict) -> bool:
    return a["stock_mint"] > 0 or a["stock_hdr"] > 0


def print_file_status(name: str, a: dict) -> None:
    print(f"  [{name}]")
    print(f"    重试铸 UUID（官方 mint）: {a['stock_mint']}  已去掉: {a['patched_mint']}")
    print(f"    header 仍 attempt≠original: {a['stock_hdr']}  已强制相同: {a['patched_hdr']}")
    print(
        f"    慢速池 dump 锚点: {a['stock_slow_dump']}  已注入: {a['patched_slow_dump']}"
    )
    print(f"    自动重试 gate 仍开启: {a['gate_stock']}")
    print(f"    配置默认开启重试: {a['default_on']}  已改为关闭: {a['default_off']}")
    marks = []
    if a["marker_reuse_v2"]:
        marks.append(f"reuse:{pver}")
    elif a["marker_reuse"]:
        marks.append("reuse")
    if a["marker_disable"]:
        marks.append("disable")
    if a["marker_slow_dump"]:
        marks.append("slow-dump")
    print(f"    标记: {('、'.join(marks) if marks else '无')}")
    if a["stock_mint"] not in (0, EXPECT_MINT_PER_FILE) and a["stock_mint"] > 0:
        print(f"    ⚠ mint 命中数={a['stock_mint']}（常见为 {EXPECT_MINT_PER_FILE}），升级后请核对 dry-run 片段")
    if a["stock_hdr"] not in (0, EXPECT_HDR_PER_FILE) and a["stock_hdr"] > 0:
        print(f"    ⚠ header 命中数={a['stock_hdr']}（常见为 {EXPECT_HDR_PER_FILE}）")
    if a["stock_slow_dump"] not in (0, EXPECT_SLOW_DUMP_PER_FILE) and a["stock_slow_dump"] > 0:
        print(f"    ⚠ slow-dump 命中数={a['stock_slow_dump']}（常见为 {EXPECT_SLOW_DUMP_PER_FILE}）")


def show_snippets(name: str, text: str, a: dict, limit: int = 4) -> None:
    print(f"  --- {name} 将修改的片段 ---")
    n = 0
    for m in a["_stock_mint_ms"]:
        if n >= limit:
            break
        s = max(0, m.start() - 40)
        e = min(len(text), m.end() + 40)
        print(f"    mint: ...{text[s:e]}...")
        n += 1
    for m in a["_stock_hdr_ms"]:
        if n >= limit:
            break
        s = max(0, m.start() - 20)
        e = min(len(text), m.end() + 20)
        print(f"    hdr:  ...{text[s:e]}...")
        n += 1
    for m in a["_stock_slow_ms"]:
        if n >= limit:
            break
        s = max(0, m.start() - 20)
        e = min(len(text), m.start() + 120)
        print(f"    slow: ...{text[s:e]}...")
        n += 1
    if n == 0:
        print("    （无可改官方特征；可能已打补丁或特征已变）")


def conclude(ad: dict, ag: dict) -> None:
    print("---- 结论 ----")
    d_ok, g_ok = reuse_effective(ad), reuse_effective(ag)
    d_part, g_part = reuse_partial(ad), reuse_partial(ag)
    if d_ok and g_ok:
        print("· reuse（双保险）: 已生效")
        print("  - 重试不再 crypto.randomUUID() 铸 attempt id")
        print("  - 出网 x-request-id 强制等于 x-original-request-id")
        print("  - 仍保留 nal_agent_retries 自动续流 / checkpoint resume")
        print("  - 说明: requestTraces 里 requestId 本就绑 generation；验效应看代理 header 或 retry 日志")
    elif d_part or g_part or (ad["marker_reuse"] or ag["marker_reuse"]):
        print("· reuse: 部分生效（缺层）→ 建议再执行 apply 补齐 header 或 mint")
        print(f"  desktop effective={d_ok} partial={d_part}  glass effective={g_ok} partial={g_part}")
    elif ad["stock_mint"] + ag["stock_mint"] > 0 or ad["stock_hdr"] + ag["stock_hdr"] > 0:
        print("· reuse: 未打 → 官方重试会铸新 attempt x-request-id")
    else:
        print("· reuse: 无法判断（特征串对不上，Cursor 可能已改打包代码）")

    gate = ad["gate_stock"] + ag["gate_stock"]
    don = ad["default_on"] + ag["default_on"]
    disable_on = (ad["marker_disable"] or ag["marker_disable"]) and gate == 0
    if disable_on:
        print("· disable-retries: 已生效（激进；断流后不自动重试）")
    elif gate > 0 or don > 0:
        print("· disable-retries: 未打（默认不必打）")
    else:
        print("· disable-retries: 无法判断")

    sd_ok = slow_dump_effective(ad) and slow_dump_effective(ag)
    if sd_ok:
        print("· slow-pool-dump: 已生效（txt）")
        print(f"  path: {slow_pool_dump_path}")
        print("  - 拉取 usage limit status 后写入纯文本")
        print("  - 字段: allowedModelIds/Tags、isInSlowPool、stage、grants…")
        print("  - Console 会打印 path=…；兜底 localStorage / globalThis.__sanshuSlowPool")
    elif ad["marker_slow_dump"] or ag["marker_slow_dump"] or ad["stock_slow_dump"] or ag["stock_slow_dump"]:
        print(
            f"· slow-pool-dump: "
            f"desktop stock/patched={ad['stock_slow_dump']}/{ad['patched_slow_dump']} "
            f"glass={ag['stock_slow_dump']}/{ag['patched_slow_dump']}"
        )
    else:
        print("· slow-pool-dump: 无法判断（performFetch 特征可能已变）")

    print("主目标: reuse 双保险；slow-pool-dump 可选落盘；disable 仅在明确不要自动重试时使用。")


def atomic_write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
            f.write(content)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def restore_from_backup(bdir: str, desktop: Path, glass: Path) -> None:
    b = Path(bdir)
    atomic_write(desktop, load(b / "workbench.desktop.main.js"))
    atomic_write(glass, load(b / "workbench.glass.main.js"))


def load_pair(desktop: Path, glass: Path):
    return load(desktop), load(glass)


def file_pair_from_env():
    d = Path(os.environ["DESKTOP"])
    g = Path(os.environ["GLASS"])
    return d, g


def print_paths(desktop: Path, glass: Path):
    print(f"Cursor 版本: {ver}")
    print(f"desktop: {desktop}")
    print(f"  大小={desktop.stat().st_size}  sha={sha16(desktop)}...")
    print(f"glass:   {glass}")
    print(f"  大小={glass.stat().st_size}  sha={sha16(glass)}...")


# -------------------- selftest against fixture --------------------
if cmd == "selftest":
    fix = os.environ.get("FIXTURE_DIR") or ""
    if not fix:
        # latest backup under scripts/cursor-patch/backups if looks stock
        print("错误: selftest 需要 FIXTURE_DIR", file=sys.stderr)
        sys.exit(2)
    fixp = Path(fix)
    fd, fg = fixp / "workbench.desktop.main.js", fixp / "workbench.glass.main.js"
    if not fd.is_file() or not fg.is_file():
        print(f"错误: fixture 不完整: {fix}", file=sys.stderr)
        sys.exit(2)
    td, tg = load(fd), load(fg)
    ad, ag = analyze(td), analyze(tg)
    print(f"fixture: {fix}")
    print(f"cursor_version(meta 不强制): {ver}")
    print_file_status("desktop(stock fixture)", ad)
    print_file_status("glass(stock fixture)", ag)

    # Must look like stock (or already partial is ok for upgrade test)
    if ad["stock_mint"] == 0 and ad["stock_hdr"] == 0 and ag["stock_mint"] == 0 and ag["stock_hdr"] == 0:
        # maybe fixture is already patched — still verify idempotency
        print("fixture 无官方特征，改为幂等性自检…")
        nd, s1 = apply_reuse_layers(td)
        ng, s2 = apply_reuse_layers(tg)
        nd, s1b = apply_slow_dump_layer(nd)
        ng, s2b = apply_slow_dump_layer(ng)
        assert s1["mint"] + s1["hdr"] + s1b["slow_dump"] == 0, s1 | s1b
        assert s2["mint"] + s2["hdr"] + s2b["slow_dump"] == 0, s2 | s2b
        assert nd == td and ng == tg
        print("OK: 已补丁 fixture 上 apply 幂等")
        sys.exit(0)

    if ad["stock_mint"] < 1 or ag["stock_mint"] < 1:
        print("失败: fixture mint 特征不足", file=sys.stderr)
        sys.exit(2)
    if ad["stock_hdr"] < 1 or ag["stock_hdr"] < 1:
        print("失败: fixture header 特征不足", file=sys.stderr)
        sys.exit(2)
    if ad["stock_slow_dump"] < 1 or ag["stock_slow_dump"] < 1:
        print("失败: fixture slow-dump 特征不足", file=sys.stderr)
        sys.exit(2)

    nd, s1 = apply_reuse_layers(td)
    ng, s2 = apply_reuse_layers(tg)
    nd, s1b = apply_slow_dump_layer(nd)
    ng, s2b = apply_slow_dump_layer(ng)
    ad2, ag2 = analyze(nd), analyze(ng)
    print(f"transform desktop: mint={s1['mint']} hdr={s1['hdr']} slow={s1b['slow_dump']}")
    print(f"transform glass:   mint={s2['mint']} hdr={s2['hdr']} slow={s2b['slow_dump']}")
    print_file_status("desktop(after)", ad2)
    print_file_status("glass(after)", ag2)

    ok = reuse_effective(ad2) and reuse_effective(ag2)
    if not ok:
        print("失败: 变换后未达到双保险生效状态", file=sys.stderr)
        sys.exit(2)
    if not (slow_dump_effective(ad2) and slow_dump_effective(ag2)):
        print("失败: 变换后 slow-dump 未生效", file=sys.stderr)
        sys.exit(2)

    # idempotent second pass
    nd3, s3 = apply_reuse_layers(nd)
    ng3, s4 = apply_reuse_layers(ng)
    nd3, s3b = apply_slow_dump_layer(nd3)
    ng3, s4b = apply_slow_dump_layer(ng3)
    if s3["mint"] + s3["hdr"] + s4["mint"] + s4["hdr"] + s3b["slow_dump"] + s4b["slow_dump"] != 0:
        print("失败: 二次 apply 非幂等", s3, s4, s3b, s4b, file=sys.stderr)
        sys.exit(2)
    if nd3 != nd or ng3 != ng:
        print("失败: 二次 apply 改变了内容", file=sys.stderr)
        sys.exit(2)

    # disable dry transform on copy
    dd, sd = apply_disable(nd)
    if sd["gate"] + sd["default"] == 0 and analyze(nd)["gate_stock"] > 0:
        print("失败: disable 未能改 gate", file=sys.stderr)
        sys.exit(2)

    print("OK: selftest 通过（reuse + slow-dump + 幂等 + disable 可变换）")
    sys.exit(0)


# -------------------- live file commands --------------------
desktop, glass = file_pair_from_env()
if not desktop.is_file() or not glass.is_file():
    print("错误: workbench 文件不存在", file=sys.stderr)
    sys.exit(2)

td, tg = load_pair(desktop, glass)
ad, ag = analyze(td), analyze(tg)

if cmd == "status":
    print_paths(desktop, glass)
    print_file_status("desktop", ad)
    print_file_status("glass", ag)
    conclude(ad, ag)
    sys.exit(0)

if cmd == "check":
    print_paths(desktop, glass)
    print("检查特征串（只读，不改文件）…")
    print_file_status("desktop", ad)
    print_file_status("glass", ag)
    show_snippets("desktop", td, ad)
    show_snippets("glass", tg, ag)

    already = reuse_effective(ad) and reuse_effective(ag)
    partial = reuse_partial(ad) or reuse_partial(ag) or (
        (ad["marker_reuse"] or ag["marker_reuse"]) and not already
    )
    can = reuse_applicable(ad) and reuse_applicable(ag)
    # allow apply if either side still has stock features (upgrade path)
    can_upgrade = reuse_applicable(ad) or reuse_applicable(ag)

    ok_disable = (
        ad["gate_stock"] > 0
        and ag["gate_stock"] > 0
        and ad["default_on"] > 0
        and ag["default_on"] > 0
    )
    already_disable = ad["marker_disable"] and ag["marker_disable"] and ad["gate_stock"] == 0

    print("---- 可否应用 ----")
    if already:
        print("· reuse 双保险: 已完整生效（跳过）")
    elif can_upgrade:
        print(
            f"· reuse 双保险: 可打/可补齐"
            f"（desktop mint={ad['stock_mint']} hdr={ad['stock_hdr']};"
            f" glass mint={ag['stock_mint']} hdr={ag['stock_hdr']}）【推荐】"
        )
        if partial:
            print("  （检测到部分补丁，apply 会幂等补齐缺失层）")
    else:
        print(
            f"· reuse 双保险: 不可打"
            f"（desktop mint/hdr={ad['stock_mint']}/{ad['stock_hdr']}"
            f" glass={ag['stock_mint']}/{ag['stock_hdr']}；需更新脚本）"
        )

    if already_disable:
        print("· disable-retries: 已打过（跳过）")
    elif ok_disable:
        print(f"· disable-retries: 可打（gate {ad['gate_stock']}/{ag['gate_stock']}）【可选/激进】")
    else:
        print(
            f"· disable-retries: 不可打"
            f"（gate={ad['gate_stock']}/{ag['gate_stock']}"
            f" default_on={ad['default_on']}/{ag['default_on']}）"
        )

    slow_ok = slow_dump_effective(ad) and slow_dump_effective(ag)
    slow_can = slow_dump_applicable(ad) or slow_dump_applicable(ag)
    if slow_ok:
        print(f"· slow-pool-dump: 已生效（txt）")
        print(f"  path: {slow_pool_dump_path}")
    elif slow_can:
        print(
            f"· slow-pool-dump: 可打/可补齐"
            f"（desktop={ad['stock_slow_dump']} glass={ag['stock_slow_dump']}）"
        )
        print(f"  path: {slow_pool_dump_path}")
    else:
        print(
            f"· slow-pool-dump: 不可打"
            f"（desktop/glass stock={ad['stock_slow_dump']}/{ag['stock_slow_dump']}；需更新脚本）"
        )

    conclude(ad, ag)
    if already or can_upgrade or ok_disable or already_disable or slow_ok or slow_can:
        sys.exit(0)
    sys.exit(2)

if cmd == "dry-run":
    print_paths(desktop, glass)
    print(f"dry-run: 模拟 reuse 双保险" + (" + slow-pool-dump" if with_slow_dump else "") + "（不写盘）…")
    print_file_status("desktop", ad)
    print_file_status("glass", ag)
    show_snippets("desktop", td, ad)
    show_snippets("glass", tg, ag)
    nd, s1 = apply_reuse_layers(td)
    ng, s2 = apply_reuse_layers(tg)
    s1b = s2b = {"slow_dump": 0}
    if with_slow_dump:
        nd, s1b = apply_slow_dump_layer(nd)
        ng, s2b = apply_slow_dump_layer(ng)
    ad2, ag2 = analyze(nd), analyze(ng)
    print(
        f"将替换: desktop mint={s1['mint']} hdr={s1['hdr']} slow={s1b['slow_dump']}; "
        f"glass mint={s2['mint']} hdr={s2['hdr']} slow={s2b['slow_dump']}"
    )
    print_file_status("desktop(模拟后)", ad2)
    print_file_status("glass(模拟后)", ag2)
    total = s1["mint"] + s1["hdr"] + s2["mint"] + s2["hdr"] + s1b["slow_dump"] + s2b["slow_dump"]
    reuse_done = reuse_effective(ad2) and reuse_effective(ag2)
    # reuse must stay OK if we changed mint/hdr or already OK
    if s1["mint"] + s1["hdr"] + s2["mint"] + s2["hdr"] > 0 and not reuse_done:
        print("模拟后未达双保险生效，拒绝。", file=sys.stderr)
        sys.exit(2)
    if with_slow_dump and (s1b["slow_dump"] + s2b["slow_dump"] > 0):
        if not (slow_dump_effective(ad2) and slow_dump_effective(ag2)):
            print("模拟后 slow-dump 未生效，拒绝。", file=sys.stderr)
            sys.exit(2)
    if total == 0:
        ok_reuse = reuse_effective(ad) and reuse_effective(ag)
        ok_slow = (not with_slow_dump) or (
            slow_dump_effective(ad)
            and slow_dump_effective(ag)
            and not slow_dump_needs_upgrade(td)
            and not slow_dump_needs_upgrade(tg)
        )
        if ok_reuse and ok_slow:
            print("已是目标状态，无需修改。")
            sys.exit(0)
        print("没有可替换内容（特征可能失效）。", file=sys.stderr)
        sys.exit(2)
    # if only slow changed, still require reuse already effective on live
    if not reuse_done and not (reuse_effective(ad) and reuse_effective(ag)):
        print("模拟后 reuse 未就绪，拒绝。", file=sys.stderr)
        sys.exit(2)
    print("dry-run OK：可执行 apply。")
    sys.exit(0)

if cmd == "apply-reuse":
    need_reuse = not (
        reuse_effective(ad)
        and reuse_effective(ag)
        and not (ad["stock_mint"] or ad["stock_hdr"] or ag["stock_mint"] or ag["stock_hdr"])
    )
    need_slow = with_slow_dump and (
        slow_dump_needs_upgrade(td) or slow_dump_needs_upgrade(tg)
    )
    if not need_reuse and not need_slow:
        print("reuse / slow-dump 已完整生效，无需重复", file=sys.stderr)
        sys.exit(1)

    nd, s1 = apply_reuse_layers(td)
    ng, s2 = apply_reuse_layers(tg)
    s1b = s2b = {"slow_dump": 0}
    if with_slow_dump:
        nd, s1b = apply_slow_dump_layer(nd)
        ng, s2b = apply_slow_dump_layer(ng)
    total = s1["mint"] + s1["hdr"] + s2["mint"] + s2["hdr"] + s1b["slow_dump"] + s2b["slow_dump"]
    if total == 0:
        print("失败: 未替换任何内容（特征失效或已是目标状态）", file=sys.stderr)
        sys.exit(2)

    ad2, ag2 = analyze(nd), analyze(ng)
    if need_reuse or (s1["mint"] + s1["hdr"] + s2["mint"] + s2["hdr"] > 0):
        if not (reuse_effective(ad2) and reuse_effective(ag2)):
            print("失败: 变换后 reuse 校验未通过（不会写盘）", file=sys.stderr)
            print_file_status("desktop(变换后)", ad2)
            print_file_status("glass(变换后)", ag2)
            sys.exit(2)
    elif not (reuse_effective(ad2) and reuse_effective(ag2)):
        # did not touch reuse; must still remain effective if it was
        if reuse_effective(ad) and reuse_effective(ag):
            pass
        else:
            print("失败: reuse 状态异常", file=sys.stderr)
            sys.exit(2)
    if with_slow_dump and (s1b["slow_dump"] + s2b["slow_dump"] > 0 or need_slow):
        if not (slow_dump_effective(ad2) and slow_dump_effective(ag2)):
            print("失败: 变换后 slow-dump 校验未通过（不会写盘）", file=sys.stderr)
            print_file_status("desktop(变换后)", ad2)
            print_file_status("glass(变换后)", ag2)
            sys.exit(2)

    # size sanity: mint shrink small; dump inject can grow ~3KB/file
    for label, before, after in (("desktop", td, nd), ("glass", tg, ng)):
        delta = len(after) - len(before)
        if abs(delta) > 12288:
            print(f"失败: {label} 体积变化异常 delta={delta}（拒绝写盘）", file=sys.stderr)
            sys.exit(2)
        if len(after) < len(before) * 0.99:
            print(f"失败: {label} 体积缩水过多（拒绝写盘）", file=sys.stderr)
            sys.exit(2)

    if not write:
        print("内部错误: apply-reuse 未设置 WRITE=1", file=sys.stderr)
        sys.exit(3)

    try:
        # ensure dump parent exists on host for later runtime writes
        if slow_pool_dump_path:
            Path(slow_pool_dump_path).parent.mkdir(parents=True, exist_ok=True)
        atomic_write(desktop, nd)
        atomic_write(glass, ng)
    except Exception as e:
        print(f"写盘失败: {e}", file=sys.stderr)
        if backup_dir:
            try:
                restore_from_backup(backup_dir, desktop, glass)
                print(f"已从备份回滚: {backup_dir}", file=sys.stderr)
            except Exception as e2:
                print(f"回滚也失败: {e2}", file=sys.stderr)
        sys.exit(2)

    # post-verify live files
    try:
        ad3, ag3 = analyze(load(desktop)), analyze(load(glass))
        if need_reuse or (s1["mint"] + s1["hdr"] + s2["mint"] + s2["hdr"] > 0):
            if not (reuse_effective(ad3) and reuse_effective(ag3)):
                raise RuntimeError("写盘后 reuse 校验失败")
        if with_slow_dump and (s1b["slow_dump"] + s2b["slow_dump"] > 0 or need_slow):
            if not (slow_dump_effective(ad3) and slow_dump_effective(ag3)):
                raise RuntimeError("写盘后 slow-dump 校验失败")
    except Exception as e:
        print(f"失败: {e}，尝试回滚…", file=sys.stderr)
        if backup_dir:
            restore_from_backup(backup_dir, desktop, glass)
            print(f"已回滚: {backup_dir}", file=sys.stderr)
        sys.exit(2)

    print(
        f"apply 完成: desktop mint={s1['mint']} hdr={s1['hdr']} slow={s1b['slow_dump']}; "
        f"glass mint={s2['mint']} hdr={s2['hdr']} slow={s2b['slow_dump']}; 合计 {total}"
    )
    if s1["mint"] + s1["hdr"] + s2["mint"] + s2["hdr"] > 0:
        print("说明: 保留自动重试；重试不再铸新 attempt id，且 x-request-id≡original。")
    if s1b["slow_dump"] + s2b["slow_dump"] > 0:
        print(f"说明: slow-pool-dump 已注入（txt）")
        print(f"path: {slow_pool_dump_path}")
        print("      重启 Cursor 后打开 Composer/触限即可刷新文件；Console 也会打印 path")
    print("请【完全退出】Cursor 再打开以加载 bundle。")
    sys.exit(0)

if cmd == "apply-disable":
    if ad["marker_disable"] and ag["marker_disable"] and ad["gate_stock"] == 0 and ag["gate_stock"] == 0:
        print("disable-retries 已经打过了，无需重复", file=sys.stderr)
        sys.exit(1)

    nd, s1 = apply_disable(td)
    ng, s2 = apply_disable(tg)
    total = s1["gate"] + s1["default"] + s2["gate"] + s2["default"]
    if total == 0:
        print("失败: disable 未替换任何内容", file=sys.stderr)
        sys.exit(2)

    ad2, ag2 = analyze(nd), analyze(ng)
    if ad2["gate_stock"] > 0 or ag2["gate_stock"] > 0:
        print("失败: disable 后 gate 仍在（不写盘）", file=sys.stderr)
        sys.exit(2)

    if not write:
        print("内部错误: apply-disable 未设置 WRITE=1", file=sys.stderr)
        sys.exit(3)

    try:
        atomic_write(desktop, nd)
        atomic_write(glass, ng)
    except Exception as e:
        print(f"写盘失败: {e}", file=sys.stderr)
        if backup_dir:
            try:
                restore_from_backup(backup_dir, desktop, glass)
                print(f"已从备份回滚: {backup_dir}", file=sys.stderr)
            except Exception as e2:
                print(f"回滚也失败: {e2}", file=sys.stderr)
        sys.exit(2)

    print(
        f"disable-retries 完成: desktop gate={s1['gate']} default={s1['default']}; "
        f"glass gate={s2['gate']} default={s2['default']}; 合计 {total}"
    )
    print("说明: 已关闭自动重试；长 turn 断流会停，手动 Continue 可能是新 request。")
    print("请【完全退出】Cursor 再打开以加载 bundle。")
    sys.exit(0)

print(f"未知内部命令: {cmd}", file=sys.stderr)
sys.exit(3)
PY
}

status() {
  require_files
  run_core status
  local b
  b="$(latest_backup_dir)"
  if [[ -n "$b" ]]; then
    info "最近备份: $b"
    if [[ -f "$b/meta.txt" ]]; then
      info "  $(tr '\n' ' ' <"$b/meta.txt" | sed 's/ $//')"
      local bv cv
      bv="$(backup_meta_version "$b")"
      cv="$(cursor_version)"
      if [[ -n "$bv" && "$bv" != "未知" && "$cv" != "未知" && "$bv" != "$cv" ]]; then
        warn "备份版本($bv) ≠ 当前 Cursor($cv)，restore 可能把旧 bundle 盖到新版本上"
      fi
    fi
  else
    info "最近备份: （无）"
  fi
}

check() {
  require_files
  run_core check
}

dry_run() {
  require_files
  run_core dry-run
}

selftest() {
  require_python
  local fix="${1:-}"
  if [[ -z "$fix" ]]; then
    # Prefer a backup that still has stock mint features
    local d
    for d in "$BACKUP_ROOT"/*/ ; do
      [[ -d "$d" ]] || continue
      if grep -q 'crypto.randomUUID()' "$d/workbench.desktop.main.js" 2>/dev/null \
        && grep -q 'onRetryStarting' "$d/workbench.desktop.main.js" 2>/dev/null; then
        # quick stock-ish check via python counts later
        fix="$d"
        break
      fi
    done
    # fallback: newest backup
    [[ -n "$fix" ]] || fix="$(latest_backup_dir)"
  fi
  [[ -n "$fix" && -d "$fix" ]] || die "找不到 fixture 备份目录（先对官方包 apply 一次会生成 backups/）"
  info "selftest fixture: $fix"
  FIXTURE_DIR="$fix" run_core selftest
}

apply_mode() {
  local mode="${1:-}"
  require_writable
  local b="" dry_rc=0 dry_out=""
  case "$mode" in
    reuse-attempt-id|reuse|reuse-id|default)
      # 一次 dry-run：exit 0 可写或已完成；exit 2 特征失败
      set +e
      dry_out="$(WITH_SLOW_DUMP="${WITH_SLOW_DUMP:-0}" run_core dry-run 2>&1)"
      dry_rc=$?
      set -e
      printf '%s\n' "$dry_out"
      if [[ "$dry_rc" -ne 0 ]]; then
        die "dry-run 未通过，拒绝 apply（可先 check / selftest）"
      fi
      if printf '%s\n' "$dry_out" | grep -q '已是目标状态，无需修改'; then
        info "reuse/slow-dump 已完整生效，无需重复 apply"
        return 0
      fi
      b="$(backup_once "pre-apply-reuse-$PATCH_VER")"
      mkdir -p "$ARTIFACTS_DIR"
      WRITE=1 BACKUP_DIR="$b" WITH_SLOW_DUMP="${WITH_SLOW_DUMP:-0}" run_core apply-reuse
      ;;
    dump-slow-pool|slow-pool-dump|slow-dump)
      # 只打/升级 slow-pool dump（reuse 不动）；强制 WITH_SLOW_DUMP=1
      WITH_SLOW_DUMP=1
      export WITH_SLOW_DUMP
      set +e
      dry_out="$(WITH_SLOW_DUMP=1 run_core dry-run 2>&1)"
      dry_rc=$?
      set -e
      printf '%s\n' "$dry_out"
      if printf '%s\n' "$dry_out" | grep -q '已是目标状态，无需修改'; then
        info "slow-pool-dump 已生效，无需重复"
        info "path: $SLOW_POOL_DUMP_PATH"
        return 0
      fi
      b="$(backup_once "pre-apply-slow-dump")"
      mkdir -p "$ARTIFACTS_DIR"
      local arc=0
      set +e
      WRITE=1 BACKUP_DIR="$b" WITH_SLOW_DUMP=1 run_core apply-reuse
      arc=$?
      set -e
      if [[ "$arc" -ne 0 ]]; then
        die "slow-pool-dump apply 失败（exit=$arc）"
      fi
      info "path: $SLOW_POOL_DUMP_PATH"
      ;;
    disable-retries|disable|stop-background)
      b="$(backup_once "pre-apply-disable")"
      WRITE=1 BACKUP_DIR="$b" run_core apply-disable
      ;;
    *)
      die "未知模式: $mode（可选: reuse-attempt-id | dump-slow-pool | disable-retries）"
      ;;
  esac
  info "---- apply 后状态 ----"
  status
}

show_slow_pool() {
  local candidates=(
    "$SLOW_POOL_DUMP_PATH"
    "$HOME/Library/Application Support/Cursor/sanshu-slow-pool-models.txt"
    "$HOME/Downloads/sanshu-slow-pool-models.txt"
    "/tmp/sanshu-slow-pool-models.txt"
  )
  local p found=""
  echo "查找路径:"
  for p in "${candidates[@]}"; do
    if [[ -f "$p" ]]; then
      echo "  [OK] $p"
      found="$p"
    else
      echo "  [..] $p"
    fi
  done
  if [[ -n "$found" ]]; then
    echo "---- content ($found) ----"
    cat "$found"
    echo "---- end ----"
    echo "path: $found"
    return 0
  fi
  info "尚未生成任何 dump 文件"
  info "primary: $SLOW_POOL_DUMP_PATH"
  info "请完全退出并重启 Cursor，再发一条 Composer/Agent 消息"
  info "然后看 Console 是否有 [sanshu-slow-pool]（error 级别）"
  info "备用: DevTools → localStorage.getItem('sanshu-slow-pool-models')"
  return 1
}

restore() {
  require_writable
  local b="" force=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --force) force=1 ;;
      *)
        if [[ -z "$b" ]]; then b="$1"; else die "多余参数: $1"; fi
        ;;
    esac
    shift || true
  done
  if [[ -z "$b" ]]; then
    b="$(latest_backup_dir)"
  fi
  [[ -n "$b" && -d "$b" ]] || die "在 $BACKUP_ROOT 下找不到备份"
  [[ -f "$b/workbench.desktop.main.js" ]] || die "备份不完整: $b"
  [[ -f "$b/workbench.glass.main.js" ]] || die "备份不完整: $b"

  local bv cv
  bv="$(backup_meta_version "$b")"
  cv="$(cursor_version)"
  if [[ -n "$bv" && "$bv" != "未知" && "$cv" != "未知" && "$bv" != "$cv" && "$force" -ne 1 ]]; then
    die "备份版本($bv) 与当前 Cursor($cv) 不一致，拒绝恢复。确认无误可加 --force"
  fi

  # 恢复前再备份当前（防止误 restore）
  backup_once "pre-restore" >/dev/null

  cp -p "$b/workbench.desktop.main.js" "$DESKTOP"
  cp -p "$b/workbench.glass.main.js" "$GLASS"
  info "已从备份恢复: $b"
  info "说明: 只恢复到该次快照；若快照本身已含补丁，恢复后仍可能带补丁。"
  status
}

usage() {
  cat <<EOF
用法: $0 <命令> [选项]

目标:
  网络断流触发 nal_agent_retries 自动重试时，不再铸新 attempt requestId，
  并强制出网 header: x-request-id ≡ x-original-request-id（双保险）。
  可选注入 slow-pool-dump：拉取 usage limit status 后把慢速池 allowlist 写入文件。
  保留自动重试 / checkpoint resume。

命令:
  status                 查看当前是否生效
  check                  只读检测特征 + 展示将改片段（升级后先跑）
  dry-run                模拟 reuse（默认含 slow-dump），不写盘
  apply [--mode=模式]    备份 → dry-run 门槛 → 原子写盘 → 写后校验/失败回滚
  slow-pool              打印已落盘的慢速池 dump（若有）
  selftest [备份目录]    用 backups 里的官方包做离线变换自检（不碰 live）
  restore [备份目录]     从备份恢复（默认最近一次；版本不一致需 --force）

模式:
  reuse-attempt-id       【默认】mint 去 UUID + header 强制 original（默认不附带 slow-dump；需显式 --with-slow-pool-dump）
  dump-slow-pool         仅注入慢速池模型落盘 hook
  disable-retries        【可选/激进】彻底关掉自动重试

apply 选项:
  --with-slow-pool-dump  附带注入 slow-dump（默认关，因易导致白/黑屏）
  --no-slow-pool-dump    不注入 slow-dump
  --dump-path=PATH       覆盖 dump 路径（默认 artifacts/slow-pool-models.txt）

示例:
  $0 check
  $0 dry-run
  $0 selftest
  $0 apply
  $0 apply --mode=reuse-attempt-id
  $0 apply --mode=dump-slow-pool
  $0 apply --mode=disable-retries
  $0 apply --no-slow-pool-dump
  $0 slow-pool
  $0 status
  $0 restore

注意:
  - 改完后必须【完全退出】Cursor 再打开
  - 升级 Cursor 会覆盖 bundle，需重新 check/apply
  - slow-pool dump 文件: $SLOW_POOL_DUMP_PATH
  - requestTraces 的 requestId 本就继承 generation，不能单靠它验收；
    应看代理上的 x-request-id，或 retry 诊断日志
EOF
}

main() {
  local cmd="${1:-}"
  shift || true
  case "$cmd" in
    status) status ;;
    check) check ;;
    dry-run|dryrun)
      # optional flags for dry-run
      WITH_SLOW_DUMP="${WITH_SLOW_DUMP:-0}"
      while [[ $# -gt 0 ]]; do
        case "$1" in
          --with-slow-pool-dump) WITH_SLOW_DUMP=1 ;;
          --no-slow-pool-dump) WITH_SLOW_DUMP=0 ;;
          --dump-path=*) SLOW_POOL_DUMP_PATH="${1#--dump-path=}" ;;
          *) die "未知参数: $1" ;;
        esac
        shift || true
      done
      export WITH_SLOW_DUMP SLOW_POOL_DUMP_PATH
      dry_run
      ;;
    selftest) selftest "${1:-}" ;;
    slow-pool|slowpool|show-slow-pool) show_slow_pool ;;
    apply)
      local mode="reuse-attempt-id"
      WITH_SLOW_DUMP="${WITH_SLOW_DUMP:-0}"
      while [[ $# -gt 0 ]]; do
        case "$1" in
          --mode=*) mode="${1#--mode=}" ;;
          --mode) shift; mode="${1:-}" ;;
          --with-slow-pool-dump) WITH_SLOW_DUMP=1 ;;
          --no-slow-pool-dump) WITH_SLOW_DUMP=0 ;;
          --dump-path=*) SLOW_POOL_DUMP_PATH="${1#--dump-path=}" ;;
          *) die "未知参数: $1" ;;
        esac
        shift || true
      done
      export WITH_SLOW_DUMP SLOW_POOL_DUMP_PATH
      apply_mode "$mode"
      ;;
    restore) restore "$@" ;;
    -h|--help|help|"") usage ;;
    *) die "未知命令: $cmd（可用: status | check | dry-run | apply | slow-pool | selftest | restore）" ;;
  esac
}

main "$@"
