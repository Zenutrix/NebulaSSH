# Baut NebulaSSH (Release), den Installer und - optional - das GitHub-Release.
#
#   Nur bauen:
#     powershell -ExecutionPolicy Bypass -File installer\build.ps1
#
#   Bauen + signierte update.json + auf GitHub veroeffentlichen:
#     $env:GITHUB_TOKEN = "<dein Token>"
#     powershell -ExecutionPolicy Bypass -File installer\build.ps1 -Publish
#
# Das GitHub-Repository steht in installer\github-repo.txt (z.B. "thomasschoepf/NebulaSSH").
param([switch]$Publish)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path native\Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$repo = ""
if (Test-Path installer\github-repo.txt) { $repo = (Get-Content installer\github-repo.txt -Raw).Trim() }

# Update-Adresse fest in die App einbauen: immer das neueste GitHub-Release
if ($repo) {
    $url = "https://github.com/$repo/releases/latest/download/update.json"
    [IO.File]::WriteAllText("$root\native\update_url.txt", $url)
    Write-Host "Update-Quelle: $url"
} else {
    Write-Host "Hinweis: installer\github-repo.txt fehlt - die App sucht nicht automatisch nach Updates."
}

Write-Host "NebulaSSH $version wird gebaut..."
cargo run -q -p brand --release
cargo build -p nebulassh --release
if ($LASTEXITCODE -ne 0) { throw "Build fehlgeschlagen" }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\nebulassh.exe "dist\NebulaSSH-$version-portable.exe" -Force

$makensis = @("$env:LOCALAPPDATA\tauri\NSIS\Bin\makensis.exe", "${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $makensis) { throw "NSIS (makensis.exe) nicht gefunden - bitte von https://nsis.sourceforge.io installieren." }
& $makensis /V2 /INPUTCHARSET UTF8 "/DVERSION=$version" installer\nebulassh.nsi
if ($LASTEXITCODE -ne 0) { throw "Installer-Build fehlgeschlagen" }

# Signierte update.json (Download-Adressen zeigen auf das GitHub-Release dieser Version)
if ($repo) {
    cargo run -q -p release -- manifest $version "https://github.com/$repo/releases/download/v$version" installer/release-notes.txt
    if ($LASTEXITCODE -ne 0) { throw "update.json konnte nicht signiert werden" }
}

# Alle Dateien fuer ein GitHub-Release in einem Ordner (zum Hochladen im Browser)
if ($repo) {
    $rel = "dist\github-release"
    if (Test-Path $rel) { Remove-Item $rel -Recurse -Force }
    New-Item -ItemType Directory -Force $rel | Out-Null
    Copy-Item "dist\NebulaSSH-Setup-$version.exe" $rel
    Copy-Item "dist\NebulaSSH-$version-portable.exe" $rel
    Copy-Item "dist\NebulaSSH-Setup-$version.exe" "$rel\NebulaSSH-Setup.exe"
    Copy-Item "dist\NebulaSSH-$version-portable.exe" "$rel\NebulaSSH-portable.exe"
    Copy-Item "dist\update.json" $rel
    $sums = Get-ChildItem "$rel\*.exe" | ForEach-Object { "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name }
    [IO.File]::WriteAllLines("$root\$rel\SHA256SUMS.txt", $sums)
    Write-Host "Release-Dateien fuer GitHub (Tag v$version): $rel"
}

if ($Publish) {
    if (-not $repo) { throw "installer\github-repo.txt fehlt." }
    if (-not $env:GITHUB_TOKEN) { throw "Bitte zuerst `$env:GITHUB_TOKEN setzen." }
    cargo run -q -p release -- github $version $repo installer/release-notes.txt
    if ($LASTEXITCODE -ne 0) { throw "Veroeffentlichung fehlgeschlagen" }
}

Get-ChildItem dist | Format-Table Name, @{n="MB";e={[math]::Round($_.Length/1MB,2)}}
