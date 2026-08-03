param(
    [switch]$SkipCheck,
    [switch]$SkipFrontend
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
        'src/rust/wechat/commands.rs'
        'src/rust/wechat/history.rs'
        'src/rust/wechat/mod.rs'
        'src/rust/wechat/state.rs'
    )
    rustfmt --edition 2021 --check --config skip_children=true $rustFiles
    Assert-NativeSuccess '微信通知 Rust 格式检查' $LASTEXITCODE

    if (-not $SkipCheck) {
        cargo check
        Assert-NativeSuccess 'Rust 编译检查' $LASTEXITCODE
    }

    if (-not $SkipFrontend) {
        $frontendFiles = @(
            'src/frontend/components/settings/WechatSettings.vue'
            'src/frontend/components/settings/WechatHistoryPanel.vue'
            'src/frontend/components/settings/WechatLogPanel.vue'
            'src/frontend/composables/useMcpHandler.ts'
            'src/frontend/types/wechat.ts'
        )
        foreach ($frontendFile in $frontendFiles) {
            pnpm exec eslint $frontendFile
            Assert-NativeSuccess "微信通知前端 ESLint 检查 ($frontendFile)" $LASTEXITCODE
        }

        pnpm build
        Assert-NativeSuccess '前端生产构建' $LASTEXITCODE
    }
}
finally {
    Pop-Location
}
