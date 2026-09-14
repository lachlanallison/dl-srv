#!/usr/bin/env node
/**
 * Build a clean ZIP for Mozilla Add-ons upload.
 * Uses `web-ext build` so archive paths use forward slashes (AMO rejects Windows backslashes).
 */
import fs from 'fs'
import path from 'path'
import { fileURLToPath } from 'url'
import { execSync } from 'child_process'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const extDir = path.join(repoRoot, 'extension')
const staging = path.join(extDir, 'dist', 'amo')
const distDir = path.join(extDir, 'dist')
const zipPath = path.join(distDir, 'dl-srv-firefox.zip')

const include = [
  'background.js',
  'shared.js',
  'magnet-click.js',
  'popup.js',
  'popup.html',
  'prompt.js',
  'prompt.html',
  'options.js',
  'options.html',
  'PRIVACY.md',
  'icons/icon16.png',
  'icons/icon32.png',
  'icons/icon48.png',
  'icons/icon128.png',
]

function rmrf(target) {
  if (fs.existsSync(target)) fs.rmSync(target, { recursive: true, force: true })
}

rmrf(staging)
fs.mkdirSync(staging, { recursive: true })

const manifest = JSON.parse(
  fs.readFileSync(path.join(extDir, 'manifest.json'), 'utf8'),
)
fs.writeFileSync(path.join(staging, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n')

for (const rel of include) {
  const src = path.join(extDir, rel)
  const dest = path.join(staging, rel)
  if (!fs.existsSync(src)) {
    console.error(`Missing required file: ${rel}`)
    process.exit(1)
  }
  fs.mkdirSync(path.dirname(dest), { recursive: true })
  fs.copyFileSync(src, dest)
}

rmrf(zipPath)

execSync(
  [
    'npx web-ext build',
    `--source-dir "${staging}"`,
    `--artifacts-dir "${distDir}"`,
    '--filename dl-srv-firefox.zip',
    '--overwrite-dest',
  ].join(' '),
  { cwd: extDir, stdio: 'inherit', shell: true },
)

if (!fs.existsSync(zipPath)) {
  console.error(`Expected zip not found: ${zipPath}`)
  process.exit(1)
}

// Fail the build if any entry uses backslashes (AMO general test).
const { createRequire } = await import('module')
const require = createRequire(path.join(extDir, 'package.json'))
const AdmZip = require('adm-zip')
const bad = new AdmZip(zipPath)
  .getEntries()
  .map((e) => e.entryName)
  .filter((name) => name.includes('\\'))

if (bad.length) {
  console.error('Invalid ZIP paths (backslashes):', bad.join(', '))
  process.exit(1)
}

console.log(`Built ${zipPath}`)
