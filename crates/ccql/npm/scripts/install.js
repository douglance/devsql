#!/usr/bin/env node

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const https = require('https');
const { createGunzip } = require('zlib');
const { pipeline } = require('stream');
const { promisify } = require('util');

const pipelineAsync = promisify(pipeline);

const VERSION = require('../package.json').version;
const REPO = 'douglance/ccql';

const PLATFORM_MAP = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

async function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    https.get(url, (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        downloadFile(response.headers.location, dest).then(resolve).catch(reject);
        return;
      }
      if (response.statusCode !== 200) {
        reject(new Error(`Failed to download: ${response.statusCode}`));
        return;
      }
      response.pipe(file);
      file.on('finish', () => {
        file.close();
        resolve();
      });
    }).on('error', reject);
  });
}

async function extractTarGz(tarPath, destDir) {
  const tar = require('tar');
  await tar.extract({ file: tarPath, cwd: destDir });
}

async function main() {
  const platform = `${process.platform}-${process.arch}`;
  const target = PLATFORM_MAP[platform];

  if (!target) {
    console.error(`Unsupported platform: ${platform}`);
    console.error('Please install from source: cargo install ccql');
    process.exit(1);
  }

  const binDir = path.join(__dirname, '..', 'bin');
  const binName = process.platform === 'win32' ? 'ccql.exe' : 'ccql';
  const binPath = path.join(binDir, binName);

  // Check if already installed
  if (fs.existsSync(binPath)) {
    console.log('ccql binary already exists');
    return;
  }

  const ext = process.platform === 'win32' ? 'zip' : 'tar.gz';
  const assetName = `ccql-${target}.${ext}`;
  const downloadUrl = `https://github.com/${REPO}/releases/download/v${VERSION}/${assetName}`;

  console.log(`Downloading ccql v${VERSION} for ${target}...`);

  const tmpDir = path.join(__dirname, '..', '.tmp');
  fs.mkdirSync(tmpDir, { recursive: true });
  fs.mkdirSync(binDir, { recursive: true });

  const archivePath = path.join(tmpDir, assetName);

  try {
    await downloadFile(downloadUrl, archivePath);

    if (ext === 'tar.gz') {
      execSync(`tar -xzf "${archivePath}" -C "${tmpDir}"`);
      // Find the binary in extracted files
      const extractedBin = path.join(tmpDir, `ccql-${target}`, 'ccql');
      if (fs.existsSync(extractedBin)) {
        fs.copyFileSync(extractedBin, binPath);
      } else {
        // Try alternate location
        fs.copyFileSync(path.join(tmpDir, 'ccql'), binPath);
      }
    } else {
      // Windows zip
      execSync(`powershell -Command "Expand-Archive -Path '${archivePath}' -DestinationPath '${tmpDir}'"`, { stdio: 'inherit' });
      const extractedBin = path.join(tmpDir, 'ccql.exe');
      fs.copyFileSync(extractedBin, binPath);
    }

    fs.chmodSync(binPath, 0o755);
    console.log('ccql installed successfully!');
  } catch (err) {
    console.error('Failed to install ccql:', err.message);
    console.error('Please install from source: cargo install ccql');
    process.exit(1);
  } finally {
    // Cleanup
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

main();
