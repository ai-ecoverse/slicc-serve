param(
  [string]$Version = '',
  [string]$Dir = '',
  [switch]$Help
)

try {
  & {
    $ErrorActionPreference = 'Stop'
    $PSNativeCommandUseErrorActionPreference = $false
    $ProgressPreference = 'SilentlyContinue'

    $BinName = 'scserve'
    $Package = 'slicc-serve'
    $Repo = 'ai-ecoverse/slicc-serve'
    $LocalData = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { [System.Environment]::GetFolderPath('LocalApplicationData') }
    $DataDir = Join-Path $LocalData $Package

    function Show-Usage {
      Write-Output 'usage: install.ps1 [-Version VERSION] [-Dir DIR] [-Help]'
      Write-Output ''
      Write-Output "Installs $BinName from the $Repo release, checked against its SHA256SUMS."
      Write-Output ''
      Write-Output '  -Version VERSION   the release to install (default: the latest)'
      Write-Output "  -Dir DIR           where $BinName.exe goes (default: $DataDir\bin, added to the user's PATH)"
      Write-Output '  -Help              print this usage and exit'
    }

    function Stop-Install([string]$Message) {
      throw "install.ps1: $Message"
    }

    function Get-File([string]$Url, [string]$Path) {
      try {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Path
      } catch {
        Stop-Install "could not download ${Url}: $($_.Exception.Message)"
      }
    }

    function Get-Quoted([string]$Text) {
      return "'" + $Text.Replace("'", "''") + "'"
    }

    if ($Help) {
      Show-Usage
      return
    }

    $Version = $Version -replace '^v', ''
    if (-not $Dir) { $Dir = Join-Path $DataDir 'bin' }

    $Machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $Arch = switch ($Machine) {
      'AMD64' { 'amd64' }
      'ARM64' { 'arm64' }
      default { Stop-Install "no $BinName build for $Machine" }
    }
    $Asset = "$BinName-windows-$Arch.exe"

    $Base = if ($env:SLICC_INSTALL_BASE_URL) {
      $env:SLICC_INSTALL_BASE_URL.TrimEnd('/')
    } elseif ($Version) {
      "https://github.com/$Repo/releases/download/v$Version"
    } else {
      "https://github.com/$Repo/releases/latest/download"
    }

    $Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("$BinName-install-" + [System.Guid]::NewGuid())
    New-Item -ItemType Directory -Path $Tmp | Out-Null
    try {
      $Sums = Join-Path $Tmp 'SHA256SUMS'
      Get-File "$Base/SHA256SUMS" $Sums
      $Expected = ''
      foreach ($line in Get-Content -LiteralPath $Sums) {
        if ($line -match '^([0-9a-f]{64})  \*?(.+)$' -and $Matches[2] -eq $Asset) { $Expected = $Matches[1] }
      }
      if (-not $Expected) { Stop-Install "no checksum for $Asset in $Base/SHA256SUMS" }
      $Download = Join-Path $Tmp $Asset
      Get-File "$Base/$Asset" $Download
      $Actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Download).Hash.ToLowerInvariant()
      if ($Actual -ne $Expected) { Stop-Install "checksum mismatch for ${Asset}: expected $Expected, got $Actual" }

      $Reported = & { $ErrorActionPreference = 'Continue'; & $Download --version 2>$null }
      if ($LASTEXITCODE -ne 0) { Stop-Install "$Asset does not run on this machine" }
      $Reported = "$Reported".Trim() -replace "^$BinName ", ''
      if ($Version -and $Reported -ne $Version) { Stop-Install "$Asset reports $Reported, not $Version" }

      New-Item -ItemType Directory -Force -Path $Dir | Out-Null
      $Target = Join-Path $Dir "$BinName.exe"
      if (Test-Path -LiteralPath $Target) {
        $Parked = "$Target." + [System.Guid]::NewGuid().ToString('N').Substring(0, 8) + '.old'
        Move-Item -LiteralPath $Target -Destination $Parked
        Remove-Item -LiteralPath $Parked -Force -ErrorAction SilentlyContinue
      }
      Copy-Item -LiteralPath $Download -Destination $Target
    } finally {
      Remove-Item -Recurse -Force -LiteralPath $Tmp -ErrorAction SilentlyContinue
    }

    Write-Output "$BinName $Reported installed to $Target"

    $userPath = [System.Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @(if ($userPath) { $userPath.Split(';') | Where-Object { $_ } })
    if ($parts -notcontains $Dir) {
      [System.Environment]::SetEnvironmentVariable('Path', (@($parts) + $Dir) -join ';', 'User')
      Write-Output "Added $Dir to your user PATH; open a new terminal to use $BinName"
      Write-Output "To undo: [Environment]::SetEnvironmentVariable('Path', (([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') -ne $(Get-Quoted $Dir)) -join ';', 'User')"
    }
  }
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  if ($MyInvocation.MyCommand.Path) { exit 1 }
}
