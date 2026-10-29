#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');

const pkg = require('../package.json');

const archToTarget = {
  x64: 'x86_64',
  arm64: 'aarch64',
};

const binDir = path.join(__dirname, 'bin');
const binaryPath = path.join(binDir, `computer-use-linux-${process.platform}-${process.arch}`);

function fail(message) {
  console.error(`[computer-use-linux] ${message}`);
  process.exit(1);
}

function copyLocalBinary(source) {
  fs.mkdirSync(binDir, { recursive: true });
  fs.copyFileSync(source, binaryPath);
  fs.chmodSync(binaryPath, 0o755);
  console.log(`[computer-use-linux] installed local binary from ${source}`);
}

function download(url, destination, redirects = 5) {
  return new Promise((resolve, reject) => {
    const request = https.get(url, (response) => {
      if (
        response.statusCode >= 300 &&
        response.statusCode < 400 &&
        response.headers.location &&
        redirects > 0
      ) {
        response.resume();
        const nextUrl = new URL(response.headers.location, url).toString();
        download(nextUrl, destination, redirects - 1).then(resolve, reject);
        return;
      }

      if (response.statusCode !== 200) {
        response.resume();
        const error = new Error(`download failed with HTTP ${response.statusCode}: ${url}`);
        error.statusCode = response.statusCode;
        reject(error);
        return;
      }

      const file = fs.createWriteStream(destination, { mode: 0o600 });
      response.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    });
    request.on('error', reject);
  });
}

function parseSha256(text) {
  const match = text.match(/\b[a-fA-F0-9]{64}\b/);
  if (!match) {
    throw new Error('sha256 file did not contain a 64-character hex digest');
  }
  return match[0].toLowerCase();
}

function sha256File(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

async function main() {
  if (process.env.COMPUTER_USE_LINUX_SKIP_DOWNLOAD === '1') {
    console.log('[computer-use-linux] skipping binary download');
    return;
  }

  if (process.env.COMPUTER_USE_LINUX_LOCAL_BINARY) {
    copyLocalBinary(process.env.COMPUTER_USE_LINUX_LOCAL_BINARY);
    return;
  }

  if (process.platform !== 'linux') {
    fail(`unsupported platform: ${process.platform}. This package only supports Linux.`);
  }

  const targetArch = archToTarget[process.arch];
  if (!targetArch) {
    fail(`unsupported CPU architecture: ${process.arch}. Supported: x64, arm64.`);
  }

  const asset = `computer-use-linux-${targetArch}-unknown-linux-gnu`;
  const baseUrl =
    process.env.COMPUTER_USE_LINUX_DOWNLOAD_BASE ||
    `https://github.com/agent-sh/computer-use-linux/releases/download/v${pkg.version}`;
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'computer-use-linux-'));
  const tmpBinary = path.join(tmpDir, asset);
  const tmpSha = path.join(tmpDir, `${asset}.sha256`);

  try {
    console.log(`[computer-use-linux] downloading ${asset} from ${baseUrl}`);
    await download(`${baseUrl}/${asset}`, tmpBinary);
    await download(`${baseUrl}/${asset}.sha256`, tmpSha);

    const expected = parseSha256(fs.readFileSync(tmpSha, 'utf8'));
    const actual = sha256File(tmpBinary);
    if (actual !== expected) {
      throw new Error(`sha256 mismatch for ${asset}: expected ${expected}, got ${actual}`);
    }

    fs.mkdirSync(binDir, { recursive: true });
    fs.copyFileSync(tmpBinary, binaryPath);
    fs.chmodSync(binaryPath, 0o755);
    console.log(`[computer-use-linux] installed ${asset}`);
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

main().catch((error) => fail(error.message));
