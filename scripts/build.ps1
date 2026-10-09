$root = Resolve-Path "$PSScriptRoot/.."
$binPath = "$root/bin"
$originalPath = Get-Location

Write-Host "🚀 Starting build process..."
Write-Host "==> Building app..."
cargo build --release

Write-Host "==> Cleaning and creating bin directory..."
Remove-Item -Recurse -Force $binPath -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $binPath | Out-Null

Write-Host "==> Moving binaries to bin folder..."
Copy-Item "$root/target/release/savefile-manager.exe" "$binPath/savefile-manager.exe"

Write-Host "==> Returning to original path..."
Set-Location "$originalPath"

Write-Host "✅ Build process complete."
Get-ChildItem $binPath
