#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const zlib = require('zlib');

const REPO = 'douglance/vcsql';
const BIN_NAME = 'vcsql';

function getPlatformTarget() {
  const platform = process.platform;
  const arch = process.arch;

  const targets = {
    'darwin-x64': 'x86_64-apple-darwin',
    'darwin-arm64': 'aarch64-apple-darwin',
    'linux-x64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
    'win32-x64': 'x86_64-pc-windows-msvc',
  };

  const key = `${platform}-${arch}`;
  const target = targets[key];

  if (!target) {
    console.error(`Unsupported platform: ${platform}-${arch}`);
    console.error('Supported platforms: darwin-x64, darwin-arm64, linux-x64, linux-arm64, win32-x64');
    process.exit(1);
  }

  return target;
}

function getVersion() {
  const packageJson = require('../package.json');
  return packageJson.version;
}

function getDownloadUrl(version, target) {
  const ext = target.includes('windows') ? 'zip' : 'tar.gz';
  return `https://github.com/${REPO}/releases/download/v${version}/${BIN_NAME}-${target}.${ext}`;
}

function downloadFile(url) {
  return new Promise((resolve, reject) => {
    const handleResponse = (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        https.get(response.headers.location, handleResponse).on('error', reject);
        return;
      }

      if (response.statusCode !== 200) {
        reject(new Error(`Failed to download: ${response.statusCode}`));
        return;
      }

      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.on('end', () => resolve(Buffer.concat(chunks)));
      response.on('error', reject);
    };

    https.get(url, handleResponse).on('error', reject);
  });
}

function extractTarGz(buffer, destDir) {
  const tmpFile = path.join(destDir, 'tmp.tar.gz');
  fs.writeFileSync(tmpFile, buffer);
  execSync(`tar -xzf "${tmpFile}" -C "${destDir}"`, { stdio: 'inherit' });
  fs.unlinkSync(tmpFile);
}

function extractZip(buffer, destDir) {
  const tmpFile = path.join(destDir, 'tmp.zip');
  fs.writeFileSync(tmpFile, buffer);

  // Use unzip on Unix, PowerShell on Windows
  if (process.platform === 'win32') {
    execSync(`powershell -command "Expand-Archive -Path '${tmpFile}' -DestinationPath '${destDir}' -Force"`, {
      stdio: 'inherit'
    });
  } else {
    execSync(`unzip -o "${tmpFile}" -d "${destDir}"`, { stdio: 'inherit' });
  }

  fs.unlinkSync(tmpFile);
}

async function install() {
  const target = getPlatformTarget();
  const version = getVersion();
  const url = getDownloadUrl(version, target);
  const binDir = path.join(__dirname, '..', 'bin');
  const isWindows = target.includes('windows');
  const binPath = path.join(binDir, isWindows ? `${BIN_NAME}.exe` : BIN_NAME);

  console.log(`Downloading ${BIN_NAME} v${version} for ${target}...`);

  try {
    const buffer = await downloadFile(url);

    if (!fs.existsSync(binDir)) {
      fs.mkdirSync(binDir, { recursive: true });
    }

    if (isWindows) {
      extractZip(buffer, binDir);
    } else {
      extractTarGz(buffer, binDir);
    }

    // Make binary executable on Unix
    if (!isWindows) {
      fs.chmodSync(binPath, 0o755);
    }

    console.log(`Successfully installed ${BIN_NAME} to ${binPath}`);
  } catch (error) {
    console.error(`Failed to install ${BIN_NAME}:`, error.message);
    console.error(`\nYou can install manually from: https://github.com/${REPO}/releases`);
    console.error('Or install via cargo: cargo install vcsql');
    process.exit(1);
  }
}

install();
