param(
    [switch]$SkipCheck,
    [switch]$SkipFrontend,
    [switch]$SkipBenchmark
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot

function Assert-NativeSuccess {
    param(
        [string]$Step,
        [int]$ExitCode
    )

    if ($ExitCode -ne 0) {
        throw "$Step 失败，退出码: $ExitCode"
    }
}

try {
    $rustFiles = @(
        'src/rust/app/builder.rs'
        'src/rust/config/settings.rs'
        'src/rust/mcp/tools/acemcp/commands.rs'
        'src/rust/mcp/tools/sou/fast_context.rs'
        'src/rust/mcp/tools/sou/local.rs'
        'src/rust/mcp/tools/sou/mod.rs'
    )
    rustfmt --edition 2021 --check --config skip_children=true $rustFiles
    Assert-NativeSuccess 'Rust 格式检查' $LASTEXITCODE

    cargo test --lib mcp::tools::sou
    Assert-NativeSuccess 'sou 单元测试' $LASTEXITCODE

    if (-not $SkipBenchmark) {
        cargo test --lib warm_fts5_query_p95_is_within_target_for_thousands_of_files -- --ignored --nocapture
        Assert-NativeSuccess 'Local warm p95 性能基准' $LASTEXITCODE
    }

    if (-not $SkipCheck) {
        cargo check --lib
        Assert-NativeSuccess 'Rust library 编译检查' $LASTEXITCODE
    }

    if (-not $SkipFrontend) {
        pnpm exec eslint src/frontend/components/tools/SouConfig.vue
        Assert-NativeSuccess 'SouConfig ESLint 检查' $LASTEXITCODE

        pnpm build
        Assert-NativeSuccess '前端生产构建' $LASTEXITCODE
    }
}
finally {
    Pop-Location
}
