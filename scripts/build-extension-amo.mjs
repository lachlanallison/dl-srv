#!/usr/bin/env node
/**
 * Build a clean ZIP for Mozilla Add-ons upload.
 */
import fs from 'fs'
import path from 'path'
import { fileURLToPath } from 'url'
import { execSync } from 'child_process'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const extDir = path.join(repoRoot, 'extension')
const staging = path.join(extDir, 'dist', 'amo')
const zipPath = path.join(extDir, 'dist', 'dl-srv-firefox.zip')

const include = [
  'background.js',
  'shared.js',
  'popup.js',
  'popup.html',
  'prompt.js',
  'prompt.html',
  'options.js',
  'options.html',
  'PRIVACY.md',
  'icons/icon16.png',
  'icons/icon48.png',
  'icons/icon128.png',
]

function rmrf(dir) {
  if (fs.existsSync(dir)) fs.rmSync(dir, { recursive: true, force: true })
}

rmrf(staging)
fs.mkdirSync(staging, { recursive: true })

const manifest = JSON.parse(
  fs.readFileSync(path.join(extDir, 'manifest.firefox.json'), 'utf8'),
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
if (process.platform === 'win32') {
  execSync(
    `powershell -NoProfile -Command "Compress-Archive -Path '${staging}\\*' -DestinationPath '${zipPath}' -Force"`,
    { stdio: 'inherit' },
  )
} else {
  execSync(`cd "${staging}" && zip -r "${zipPath}" .`, { stdio: 'inherit' })
}

console.log(`Built ${zipPath}`)
