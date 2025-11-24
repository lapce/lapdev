param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\Lapdev\bin",
    [string]$BaseUrl = ""
)

$ErrorActionPreference = "Stop"

function Get-TargetTriple {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64" { return "x86_64-pc-windows-msvc" }
        "Arm64" { return "aarch64-pc-windows-msvc" }
        default { throw "Unsupported CPU architecture: $arch" }
    }
}

function Get-DownloadUrl {
    param (
        [string]$BaseUrl,
        [string]$Version,
        [string]$Target
    )

    $assetName = "lapdev-cli-$Target.zip"
    if ($BaseUrl -and $BaseUrl.Trim().Length -gt 0) {
        if ($Version -eq "latest") {
            return "$BaseUrl/releases/latest/$assetName"
        }

        $normalized = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
        return "$BaseUrl/releases/$normalized/$assetName"
    }

    if ($Version -eq "latest") {
        return "https://github.com/lapce/lapdev/releases/latest/download/$assetName"
    }

    $normalized = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
    return "https://github.com/lapce/lapdev/releases/download/$normalized/$assetName"
}

$targetTriple = Get-TargetTriple
$downloadUrl = Get-DownloadUrl -BaseUrl $BaseUrl -Version $Version -Target $targetTriple

Write-Host "Downloading Lapdev CLI ($targetTriple) from $downloadUrl"

$tempZip = Join-Path -Path ([System.IO.Path]::GetTempPath()) -ChildPath ("lapdev-cli-{0}.zip" -f ([guid]::NewGuid()))
$tempDir = Join-Path -Path ([System.IO.Path]::GetTempPath()) -ChildPath ("lapdev-cli-{0}" -f ([guid]::NewGuid()))

try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $tempZip -UseBasicParsing
    Expand-Archive -Path $tempZip -DestinationPath $tempDir -Force

    $binary = Get-ChildItem -Path $tempDir -Filter "lapdev.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $binary) {
        throw "Could not locate lapdev.exe in downloaded archive."
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $destination = Join-Path -Path $InstallDir -ChildPath "lapdev.exe"
    Copy-Item -Force -Path $binary.FullName -Destination $destination
}
finally {
    if (Test-Path $tempZip) {
        Remove-Item -Path $tempZip -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path $tempDir) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$pathEntries = $env:Path -split ";"
if (-not ($pathEntries | Where-Object { $_ -eq $InstallDir })) {
    $newPath = ($pathEntries + $InstallDir | Where-Object { $_ -and ($_ -ne "") }) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "Added $InstallDir to your user PATH. Restart your shell to pick up the change."
}

Write-Host "Lapdev CLI installed to $destination"
& $destination --version
