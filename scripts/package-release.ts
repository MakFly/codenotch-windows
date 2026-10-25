// Run after committing the release sources. Build locally before packaging.
import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { zip } from './zip';

if (process.platform !== 'win32') throw new Error('The Windows portable release must be packaged on Windows.');
const root = resolve(import.meta.dir, '..');
function command(program: string, args: string[]) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8', windowsHide: true, maxBuffer: 16 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(`${program} failed: ${result.stderr || result.error?.message || result.status}`);
  return result.stdout.trim();
}
const commit = command('git', ['rev-parse', '--verify', 'HEAD']);
if (command('git', ['status', '--porcelain'])) throw new Error('Commit the release sources before packaging.');
const pkg = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
const tauri = JSON.parse(await readFile(join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
const cargo = await readFile(join(root, 'src-tauri/Cargo.toml'), 'utf8');
if (pkg.version !== tauri.version || !cargo.includes(`version = "${pkg.version}"`)) throw new Error('Release versions do not match.');
if (!tauri.plugins?.updater?.pubkey) throw new Error('Set plugins.updater.pubkey in tauri.conf.json before releasing.');
const version: string = pkg.version;
const source = join(root, 'src-tauri/target/release/smashtoken.exe');
const binary = await readFile(source);
const pe = binary.readUInt32LE(0x3c);
if (binary.toString('ascii', pe, pe+4) !== 'PE\0\0' || binary.readUInt16LE(pe+4) !== 0x8664) throw new Error('Expected a Windows x64 executable.');
const output = join(root, 'release');
const stage = join(output, `v${version}`);
await mkdir(stage, { recursive: true });
await copyFile(source, join(stage, 'CodeNotch.exe'));
await copyFile(join(root, 'LICENSE'), join(stage, 'LICENSE'));
await copyFile(join(root, 'THIRD_PARTY_NOTICES.md'), join(stage, 'THIRD_PARTY_NOTICES.md'));

const notices = [await readFile(join(root, 'LICENSE'), 'utf8'), await readFile(join(root, 'THIRD_PARTY_NOTICES.md'), 'utf8')];
const licenseName = /^(?:licen[sc]e|copying|notice)(?:[-._ ].*)?$/i;
async function licenseFiles(folder: string, explicit?: string | null) {
  const files = new Set<string>();
  for (const item of await readdir(folder, { withFileTypes: true })) {
    if (item.isFile() && licenseName.test(item.name)) files.add(join(folder, item.name));
  }
  if (explicit) {
    const path = resolve(folder, explicit);
    if (existsSync(path)) files.add(path);
  }
  return [...files].sort();
}
async function addNotice(name: string, version: string, license: string, folder: string, explicit?: string | null) {
  const files = await licenseFiles(folder, explicit);
  notices.push(`\n${'='.repeat(72)}\n${name} ${version}\nDeclared license: ${license}\n`);
  for (const file of files) notices.push(await readFile(file, 'utf8'));
}
const metadata = JSON.parse(command('cargo', ['metadata', '--locked', '--format-version', '1', '--manifest-path', 'src-tauri/Cargo.toml', '--filter-platform', 'x86_64-pc-windows-msvc']));
for (const dependency of metadata.packages.filter((item: any) => item.source).sort((a: any,b: any) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`))) {
  await addNotice(dependency.name, dependency.version, dependency.license ?? 'See license text', dirname(dependency.manifest_path), dependency.license_file);
}
const visited = new Set<string>();
async function addJavaScript(name: string, from: string) {
  let search = from;
  let file = '';
  while (true) {
    const candidate = join(search, 'node_modules', name, 'package.json');
    if (existsSync(candidate)) { file = candidate; break; }
    const parent = dirname(search);
    if (parent === search) throw new Error(`Install dependencies before packaging: ${name}`);
    search = parent;
  }
  const item = JSON.parse(await readFile(file, 'utf8'));
  const key = `${item.name}@${item.version}`;
  if (visited.has(key)) return;
  visited.add(key);
  await addNotice(item.name, item.version, typeof item.license === 'string' ? item.license : 'See license text', dirname(file));
  for (const child of Object.keys(item.dependencies ?? {}).sort()) await addJavaScript(child, dirname(file));
}
for (const name of Object.keys(pkg.dependencies).sort()) await addJavaScript(name, root);
const thirdParty = 'Dependency license texts from locally installed packages, including build-time packages for completeness.\n\n' + notices.join('\n');
await writeFile(join(stage, 'THIRD_PARTY_LICENSES.txt'), thirdParty);
await writeFile(join(stage, 'BUILD-INFO.txt'), `CodeNotch for Windows ${version}\nSource commit: ${commit}\nPlatform: windows-x64\nBinary SHA-256: ${createHash('sha256').update(binary).digest('hex')}\n`);
await writeFile(join(stage, 'GETTING-STARTED.txt'), `CodeNotch for Windows ${version}\n\nRun CodeNotch.exe. Hover over the tab at the right edge of the screen.\nHover over an account to see quotas; move away to collapse.\nThe settings wheel opens accounts and preferences.\nAppearance > Interface language: English / Francais.\nDrag the grip to remember placement.\nSign in through Codex or Claude Code before detecting local profiles.\nClosing settings keeps tracking active. Tray menu > Quit exits the app.\n\nRequires Windows 10/11 x64 and WebView2. The executable is unsigned.\nDo not disable Windows security features to bypass a block.\nThe portable build checks for signed updates and installs them with the setup program.\n\nDocumentation and source:\nhttps://github.com/MakFly/codenotch-windows\n`);
await writeFile(join(stage, 'LIRE-MOI.txt'), `CodeNotch pour Windows ${version}\n\nLancez CodeNotch.exe et survolez la languette au bord droit.\nLa roue ouvre les reglages. Pour le francais :\nAppearance > Interface language > Francais.\nLe detail se referme lorsque la souris quitte l'encoche.\nDeplacez la poignee pour memoriser la position.\nConnectez Codex ou Claude Code avant de detecter les profils locaux.\nFermer les reglages laisse le suivi actif. Icone pres de l'horloge > Quitter.\n\nWindows 10/11 x64 et WebView2 requis. Version portable non signee.\nNe desactivez pas les protections Windows pour contourner un blocage.\n\nDocumentation : https://github.com/MakFly/codenotch-windows\n`);
const contents = ['CodeNotch.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt', 'BUILD-INFO.txt', 'GETTING-STARTED.txt', 'LIRE-MOI.txt'];
const zipPath = join(output, `CodeNotch-${version}-windows-x64.zip`);
const entries = await Promise.all(contents.map(async name => ({ name, data: await readFile(join(stage, name)) })));
await writeFile(zipPath, zip(entries));
await copyFile(join(stage, 'THIRD_PARTY_LICENSES.txt'), join(output, 'THIRD_PARTY_LICENSES.txt'));
// Signed NSIS installer and the feed read by the in-app updater (tauri.conf.json > plugins.updater).
// Build it with TAURI_SIGNING_PRIVATE_KEY set so Tauri writes the .sig next to the installer.
const nsis = join(root, 'src-tauri/target/release/bundle/nsis');
const built = existsSync(nsis) ? (await readdir(nsis)).find(name => name.endsWith(`_${version}_x64-setup.exe`)) : undefined;
if (!built || !existsSync(join(nsis, `${built}.sig`))) throw new Error('Build the signed installer first: set TAURI_SIGNING_PRIVATE_KEY, then run bun run desktop:build.');
const setupName = `CodeNotch-${version}-windows-x64-setup.exe`;
await copyFile(join(nsis, built), join(output, setupName));
const signature = (await readFile(join(nsis, `${built}.sig`), 'utf8')).trim();
const download = { signature, url: `https://github.com/MakFly/codenotch-windows/releases/download/v${version}/${setupName}` };
await writeFile(join(output, 'latest.json'), JSON.stringify({
  version,
  notes: `https://github.com/MakFly/codenotch-windows/releases/tag/v${version}`,
  pub_date: new Date().toISOString(),
  platforms: { 'windows-x86_64': download, 'windows-x86_64-nsis': download },
}, null, 2) + '\n');
const assets = [join(output, setupName), join(output, 'latest.json'), join(stage, 'CodeNotch.exe'), zipPath, join(output, 'THIRD_PARTY_LICENSES.txt')];
const sums = [];
for (const asset of assets) sums.push(`${createHash('sha256').update(await readFile(asset)).digest('hex')}  ${asset.split(/[\\/]/).pop()}`);
await writeFile(join(output, 'SHA256SUMS.txt'), sums.join('\n') + '\n');
console.log(`Packaged CodeNotch ${version} from ${commit.slice(0,7)}; ${metadata.packages.length-1} Rust packages and ${visited.size} JS packages documented.`);
