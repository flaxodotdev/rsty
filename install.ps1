# rsty installer for Windows.
#   irm https://raw.githubusercontent.com/flaxodotdev/rsty/main/install.ps1 | iex
# Env: RSTY_VERSION (e.g. v0.1.0, default latest), RSTY_INSTALL_DIR (default %LOCALAPPDATA%\Programs\rsty)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'flaxodotdev/rsty'
$version = if ($env:RSTY_VERSION) { $env:RSTY_VERSION } else { 'latest' }
$dir = if ($env:RSTY_INSTALL_DIR) { $env:RSTY_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\rsty' }

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'x86_64' }
    'ARM64' { 'aarch64' }
    default { throw "unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}
$target = "$arch-pc-windows-msvc"
$file = "rsty-$target.zip"
$base = if ($version -eq 'latest') { "https://github.com/$repo/releases/latest/download" }
        else { "https://github.com/$repo/releases/download/$version" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("rsty-" + [guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Write-Host "rsty: downloading $file ($version)..." -ForegroundColor DarkYellow
    Invoke-WebRequest "$base/$file" -OutFile "$tmp\$file" -UseBasicParsing

    $haveSum = $true
    try { Invoke-WebRequest "$base/$file.sha256" -OutFile "$tmp\$file.sha256" -UseBasicParsing }
    catch { $haveSum = $false }
    if ($haveSum) {
        $expected = ((Get-Content "$tmp\$file.sha256" -Raw).Trim() -split '\s+')[0].ToLower()
        $actual = (Get-FileHash "$tmp\$file" -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) { throw "checksum mismatch for $file" }
        Write-Host "rsty: checksum ok" -ForegroundColor DarkYellow
    }

    Expand-Archive "$tmp\$file" -DestinationPath "$tmp\out" -Force
    New-Item -ItemType Directory $dir -Force | Out-Null
    Copy-Item "$tmp\out\rsty.exe" (Join-Path $dir 'rsty.exe') -Force
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $dir)) {
    [Environment]::SetEnvironmentVariable('Path', "$dir;$userPath", 'User')
    $env:Path = "$dir;$env:Path"
    Write-Host "rsty: added $dir to your user PATH (restart other terminals)" -ForegroundColor DarkYellow
}
Write-Host "rsty: installed to $dir\rsty.exe. Run: rsty" -ForegroundColor Green
