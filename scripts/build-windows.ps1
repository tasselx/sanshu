param(
    [ValidateSet("Debug", "Release", "All")]
    [string]$Mode = "All",

    [switch]$NoAlias
)

$ErrorActionPreference = "Stop"

# 中文说明：统一使用 UTF-8，避免中文可执行文件名和日志在 Windows 终端中乱码。
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding

$ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$InfoDir = Join-Path $ProjectRoot "target\build-info"
$DistDir = Join-Path $ProjectRoot "dist"

Set-Location -LiteralPath $ProjectRoot

function Test-CommandExists {
    param([Parameter(Mandatory = $true)][string]$Command)
    return [bool](Get-Command $Command -ErrorAction SilentlyContinue)
}

function Assert-CommandExists {
    param([Parameter(Mandatory = $true)][string]$Command)

    if (-not (Test-CommandExists $Command)) {
        throw "未找到命令：$Command，请先安装后再运行构建脚本。"
    }
}

function Invoke-NativeCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    Write-Host ""
    Write-Host "==> $Title" -ForegroundColor Cyan
    Write-Host "    $Command $($Arguments -join ' ')" -ForegroundColor DarkGray

    & $Command @Arguments

    if ($LASTEXITCODE -ne 0) {
        throw "$Title 失败，退出码：$LASTEXITCODE"
    }
}

function Get-ToolVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [string[]]$Arguments = @("--version")
    )

    if (-not (Test-CommandExists $Command)) {
        return "not found"
    }

    try {
        $output = & $Command @Arguments 2>$null | Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($output)) {
            return "unknown"
        }
        return $output.ToString().Trim()
    }
    catch {
        return "unknown"
    }
}

function Get-GitValue {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    if (-not (Test-CommandExists "git")) {
        return $null
    }

    try {
        $output = & git @Arguments 2>$null | Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($output)) {
            return $null
        }
        return $output.ToString().Trim()
    }
    catch {
        return $null
    }
}

function Get-CargoPackageVersion {
    $cargoToml = Join-Path $ProjectRoot "Cargo.toml"
    $content = Get-Content -LiteralPath $cargoToml -Raw

    if ($content -match '(?m)^\s*version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }

    return "unknown"
}

function Get-JsonVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$PropertyName
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return "missing"
    }

    try {
        $json = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
        $value = $json.$PropertyName
        if ([string]::IsNullOrWhiteSpace($value)) {
            return "unknown"
        }
        return $value
    }
    catch {
        return "unknown"
    }
}

function Get-DirectorySummary {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{
            path       = $Path
            exists     = $false
            file_count = 0
            size_bytes = 0
            size_mb    = 0
        }
    }

    $files = @(Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue)
    $sizeBytes = ($files | Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sizeBytes) {
        $sizeBytes = 0
    }

    return [pscustomobject]@{
        path       = (Resolve-Path -LiteralPath $Path).Path
        exists     = $true
        file_count = $files.Count
        size_bytes = [int64]$sizeBytes
        size_mb    = [math]::Round($sizeBytes / 1MB, 2)
    }
}

function Get-ArtifactInfo {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{
            name            = [System.IO.Path]::GetFileName($Path)
            path            = $Path
            exists          = $false
            size_bytes      = 0
            size_mb         = 0
            last_write_time = $null
            md5             = $null
            sha256          = $null
        }
    }

    $item = Get-Item -LiteralPath $Path
    $md5 = (Get-FileHash -LiteralPath $Path -Algorithm MD5).Hash.ToLowerInvariant()
    $sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()

    return [pscustomobject]@{
        name            = $item.Name
        path            = $item.FullName
        exists          = $true
        size_bytes      = [int64]$item.Length
        size_mb         = [math]::Round($item.Length / 1MB, 2)
        last_write_time = $item.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss zzz")
        md5             = $md5
        sha256          = $sha256
    }
}

function Write-BuildInfoMarkdown {
    param(
        [Parameter(Mandatory = $true)]$Report,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $lines = @(
        "# Windows 构建信息",
        "",
        "## 基础信息",
        "",
        "- 构建模式：$($Report.mode)",
        "- 项目目录：$($Report.project_root)",
        "- 前端目录：$($Report.frontend_dist.path)",
        "- 后端目录：$($Report.backend_target_dir)",
        "- 生成时间：$($Report.generated_at)",
        "",
        "## 版本信息",
        "",
        "- Cargo：$($Report.versions.cargo)",
        "- Tauri：$($Report.versions.tauri)",
        "- package.json：$($Report.versions.package_json)",
        "",
        "## Git 信息",
        "",
        "- 分支：$($Report.git.branch)",
        "- 提交：$($Report.git.commit)",
        "- 状态：$($Report.git.status)",
        "",
        "## 工具链",
        "",
        "- pnpm：$($Report.toolchain.pnpm)",
        "- node：$($Report.toolchain.node)",
        "- cargo：$($Report.toolchain.cargo)",
        "- rustc：$($Report.toolchain.rustc)",
        "",
        "## 产物 Hash",
        ""
    )

    foreach ($artifact in $Report.artifacts) {
        $lines += @(
            "### $($artifact.name)",
            "",
            "- 路径：$($artifact.path)",
            "- 大小：$($artifact.size_mb) MB ($($artifact.size_bytes) bytes)",
            "- MD5：$($artifact.md5)",
            "- SHA256：$($artifact.sha256)",
            ""
        )
    }

    Set-Content -LiteralPath $Path -Value $lines -Encoding UTF8
}

function New-BuildReport {
    param(
        [Parameter(Mandatory = $true)][string]$BuildMode,
        [Parameter(Mandatory = $true)][string]$TargetDir,
        [Parameter(Mandatory = $true)]$Artifacts
    )

    $tauriConfigPath = Join-Path $ProjectRoot "tauri.conf.json"
    $packageJsonPath = Join-Path $ProjectRoot "package.json"
    $cargoVersion = Get-CargoPackageVersion
    $tauriVersion = Get-JsonVersion -Path $tauriConfigPath -PropertyName "version"
    $packageVersion = Get-JsonVersion -Path $packageJsonPath -PropertyName "version"
    $branch = Get-GitValue -Arguments @("rev-parse", "--abbrev-ref", "HEAD")
    $commit = Get-GitValue -Arguments @("rev-parse", "--short=12", "HEAD")
    $dirty = Get-GitValue -Arguments @("status", "--short")
    $gitStatus = if ([string]::IsNullOrWhiteSpace($dirty)) { "clean" } else { "dirty" }

    return [pscustomobject]@{
        mode               = $BuildMode
        generated_at       = (Get-Date).ToString("yyyy-MM-dd HH:mm:ss zzz")
        project_root       = $ProjectRoot
        backend_target_dir = $TargetDir
        frontend_dist      = Get-DirectorySummary -Path $DistDir
        versions           = [pscustomobject]@{
            cargo        = $cargoVersion
            tauri        = $tauriVersion
            package_json = $packageVersion
        }
        git                = [pscustomobject]@{
            branch = $(if ($branch) { $branch } else { "unknown" })
            commit = $(if ($commit) { $commit } else { "unknown" })
            status = $gitStatus
        }
        toolchain          = [pscustomobject]@{
            pnpm  = Get-ToolVersion -Command "pnpm"
            node  = Get-ToolVersion -Command "node"
            cargo = Get-ToolVersion -Command "cargo"
            rustc = Get-ToolVersion -Command "rustc"
        }
        artifacts          = $Artifacts
    }
}

function Invoke-BackendBuild {
    param([Parameter(Mandatory = $true)][ValidateSet("Debug", "Release")][string]$BuildMode)

    $profileName = if ($BuildMode -eq "Release") { "release" } else { "debug" }
    $targetDir = Join-Path $ProjectRoot "target\$profileName"
    $cargoArgs = @("build", "--bins", "--features", "custom-protocol")

    if ($BuildMode -eq "Release") {
        $cargoArgs = @("build", "--release", "--bins", "--features", "custom-protocol")
    }

    Invoke-NativeCommand -Title "编译后端 ($BuildMode)" -Command "cargo" -Arguments $cargoArgs

    $uiExe = Join-Path $targetDir "等一下.exe"
    $mcpExe = Join-Path $targetDir "三术.exe"
    $aliasExe = Join-Path $targetDir "sanshu.exe"

    if (-not (Test-Path -LiteralPath $uiExe)) {
        throw "未找到 UI 产物：$uiExe"
    }
    if (-not (Test-Path -LiteralPath $mcpExe)) {
        throw "未找到 MCP 产物：$mcpExe"
    }

    if (-not $NoAlias) {
        # 中文说明：生成 ASCII 兼容入口，便于从构建目录直接验证 MCP 客户端配置。
        Copy-Item -LiteralPath $mcpExe -Destination $aliasExe -Force
    }

    $artifactPaths = @($uiExe, $mcpExe)
    if (-not $NoAlias) {
        $artifactPaths += $aliasExe
    }

    $artifacts = @($artifactPaths | ForEach-Object { Get-ArtifactInfo -Path $_ })
    $report = New-BuildReport -BuildMode $BuildMode -TargetDir $targetDir -Artifacts $artifacts

    New-Item -ItemType Directory -Path $InfoDir -Force | Out-Null
    $jsonPath = Join-Path $InfoDir "windows-build-info-$($BuildMode.ToLowerInvariant()).json"
    $markdownPath = Join-Path $InfoDir "windows-build-info-$($BuildMode.ToLowerInvariant()).md"

    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $jsonPath -Encoding UTF8
    Write-BuildInfoMarkdown -Report $report -Path $markdownPath

    Write-Host ""
    Write-Host "[$BuildMode] 构建完成" -ForegroundColor Green
    Write-Host "  后端目录: $targetDir"
    Write-Host "  前端目录: $($report.frontend_dist.path)"
    Write-Host "  版本号: Cargo=$($report.versions.cargo), Tauri=$($report.versions.tauri), package.json=$($report.versions.package_json)"
    Write-Host "  Git: $($report.git.branch)@$($report.git.commit) ($($report.git.status))"
    Write-Host "  信息文件: $jsonPath"
    Write-Host "  Markdown: $markdownPath"

    foreach ($artifact in $artifacts) {
        Write-Host ""
        Write-Host "  $($artifact.name)" -ForegroundColor Yellow
        Write-Host "    路径: $($artifact.path)"
        Write-Host "    大小: $($artifact.size_mb) MB"
        Write-Host "    MD5: $($artifact.md5)"
        Write-Host "    SHA256: $($artifact.sha256)"
    }
}

try {
    if ($env:OS -ne "Windows_NT") {
        Write-Warning "当前环境不是 Windows，脚本仍会尝试执行，但产物文件名按 Windows .exe 检查。"
    }

    Assert-CommandExists "pnpm"
    Assert-CommandExists "cargo"

    Write-Host "三术 Windows 编译脚本" -ForegroundColor Green
    Write-Host "项目目录: $ProjectRoot"
    Write-Host "构建模式: $Mode"

    Invoke-NativeCommand -Title "编译前端" -Command "pnpm" -Arguments @("build")

    $buildModes = if ($Mode -eq "All") { @("Debug", "Release") } else { @($Mode) }
    foreach ($buildMode in $buildModes) {
        Invoke-BackendBuild -BuildMode $buildMode
    }

    Write-Host ""
    Write-Host "全部构建完成。" -ForegroundColor Green
}
catch {
    Write-Host ""
    Write-Host "构建失败: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
