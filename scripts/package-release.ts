// Run after committing the release sources and `cargo build --release` in gpui-app.
// Produces the portable zip, the signed NSIS installer and the feed read by the in-app
// updater (latest.json), which both the GPUI build and the 0.1.x Tauri builds follow.
import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { zip } from './zip';

if (process.platform !== 'win32') throw new Error('The Windows release must be packaged on Windows.');
const root = resolve(import.meta.dir, '..');
function command(program: string, args: string[]) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8', windowsHide: true, maxBuffer: 16 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(`${program} failed: ${result.stderr || result.stdout || result.error?.message || result.status}`);
  return result.stdout.trim();
}
const commit = command('git', ['rev-parse', '--verify', 'HEAD']);
if (command('git', ['status', '--porcelain'])) throw new Error('Commit the release sources before packaging.');
const pkg = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
const cargo = await readFile(join(root, 'gpui-app/Cargo.toml'), 'utf8');
if (!cargo.includes(`version = "${pkg.version}"`)) throw new Error('Release versions do not match (package.json, gpui-app/Cargo.toml).');
const version: string = pkg.version;
const source = join(root, 'gpui-app/target/release/codenotch-gpui.exe');
if (!existsSync(source)) throw new Error('Build first: cargo build --release in gpui-app.');
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
const metadata = JSON.parse(command('cargo', ['metadata', '--locked', '--format-version', '1', '--manifest-path', 'gpui-app/Cargo.toml', '--filter-platform', 'x86_64-pc-windows-msvc']));
for (const dependency of metadata.packages.filter((item: any) => item.source).sort((a: any,b: any) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`))) {
  await addNotice(dependency.name, dependency.version, dependency.license ?? 'See license text', dirname(dependency.manifest_path), dependency.license_file);
}
const thirdParty = 'Dependency license texts from locally installed packages, including build-time packages for completeness.\n\n' + notices.join('\n');
await writeFile(join(stage, 'THIRD_PARTY_LICENSES.txt'), thirdParty);
await writeFile(join(stage, 'BUILD-INFO.txt'), `CodeNotch for Windows ${version}\nSource commit: ${commit}\nPlatform: windows-x64\nBinary SHA-256: ${createHash('sha256').update(binary).digest('hex')}\n`);
await writeFile(join(stage, 'GETTING-STARTED.txt'), `CodeNotch for Windows ${version}\n\nRun CodeNotch.exe. Hover over the tab at the right edge of the screen.\nHover over an account to see quotas; move away to collapse.\nThe settings window opens from the tray icon or the notch.\nAppearance > Interface language: English / Francais.\nClosing settings keeps tracking active. Tray menu > Quit exits the app.\n\nRequires Windows 10/11 x64. The executable is not code-signed.\nDo not disable Windows security features to bypass a block.\nCodeNotch checks for signed updates and installs them with the setup program.\n\nDocumentation and source:\nhttps://github.com/MakFly/codenotch-windows\n`);
await writeFile(join(stage, 'LIRE-MOI.txt'), `CodeNotch pour Windows ${version}\n\nLancez CodeNotch.exe et survolez la languette au bord droit.\nLes reglages s'ouvrent depuis l'icone pres de l'horloge ou l'encoche.\nApparence > Langue de l'interface : Francais / English.\nLe detail se referme lorsque la souris quitte l'encoche.\nFermer les reglages laisse le suivi actif. Icone pres de l'horloge > Quitter.\n\nWindows 10/11 x64 requis. Executable non signe.\nNe desactivez pas les protections Windows pour contourner un blocage.\nCodeNotch verifie les mises a jour signees et les installe avec le programme d'installation.\n\nDocumentation : https://github.com/MakFly/codenotch-windows\n`);
const contents = ['CodeNotch.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt', 'BUILD-INFO.txt', 'GETTING-STARTED.txt', 'LIRE-MOI.txt'];
const zipPath = join(output, `CodeNotch-${version}-windows-x64.zip`);
const entries = await Promise.all(contents.map(async name => ({ name, data: await readFile(join(stage, name)) })));
await writeFile(zipPath, zip(entries));
await copyFile(join(stage, 'THIRD_PARTY_LICENSES.txt'), join(output, 'THIRD_PARTY_LICENSES.txt'));

// NSIS installer from gpui-app/installer: it installs over the Tauri builds, so 0.1.x users
// move to this one through their own updater. makensis ships with the Tauri CLI's NSIS kit.
const makensis = join(process.env.LOCALAPPDATA ?? '', 'tauri/NSIS/Bin/makensis.exe');
if (!existsSync(makensis)) throw new Error(`makensis not found: the installer needs the NSIS kit (with its nsis_tauri_utils plugin) that the Tauri CLI downloads to ${makensis}.`);
const setupName = `CodeNotch-${version}-windows-x64-setup.exe`;
const setup = join(output, setupName);
command(makensis, ['-V2', `-DVERSION=${version}`, `-DMAINBINARYSRCPATH=${source}`, `-DOUTFILE=${setup}`, join(root, 'gpui-app/installer/installer.nsi')]);
// The updaters only accept an installer signed with the key in gpui-app/src/updater.rs,
// for the version the feed announces.
const key = process.env.TAURI_SIGNING_PRIVATE_KEY_PATH ?? join(homedir(), '.tauri/codenotch.key');
if (!existsSync(key)) throw new Error('Set TAURI_SIGNING_PRIVATE_KEY_PATH to the updater signing key.');
command('bunx', ['tauri', 'signer', 'sign', '-f', key, '-p', process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '', '--app-version', version, setup]);
const signature = (await readFile(`${setup}.sig`, 'utf8')).trim();
const download = { signature, url: `https://github.com/MakFly/codenotch-windows/releases/download/v${version}/${setupName}` };
await writeFile(join(output, 'latest.json'), JSON.stringify({
  version,
  notes: `https://github.com/MakFly/codenotch-windows/releases/tag/v${version}`,
  pub_date: new Date().toISOString(),
  platforms: { 'windows-x86_64': download, 'windows-x86_64-nsis': download },
}, null, 2) + '\n');
const assets = [setup, join(output, 'latest.json'), join(stage, 'CodeNotch.exe'), zipPath, join(output, 'THIRD_PARTY_LICENSES.txt')];
const sums = [];
for (const asset of assets) sums.push(`${createHash('sha256').update(await readFile(asset)).digest('hex')}  ${asset.split(/[\\/]/).pop()}`);
await writeFile(join(output, 'SHA256SUMS.txt'), sums.join('\n') + '\n');
console.log(`Packaged CodeNotch ${version} from ${commit.slice(0,7)}; ${metadata.packages.length-1} Rust packages documented.`);
