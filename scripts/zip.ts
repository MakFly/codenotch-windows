import { deflateRawSync } from 'node:zlib';

// Small UTF-8 ZIP writer for the fixed portable bundle. No external archiver.
export function zip(files: { name: string; data: Buffer }[]): Buffer {
  const table = Array.from({ length: 256 }, (_, value) => {
    for (let i = 0; i < 8; i++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0);
    return value >>> 0;
  });
  const local: Buffer[] = [], central: Buffer[] = [];
  let offset = 0;
  for (const file of files) {
    if (file.data.length >= 0xffffffff) throw new Error('ZIP64 is not supported for release assets.');
    const name = Buffer.from(file.name, 'utf8');
    const compressed = deflateRawSync(file.data);
    let crc = 0xffffffff;
    for (const byte of file.data) crc = table[(crc ^ byte) & 255] ^ (crc >>> 8);
    crc = (crc ^ 0xffffffff) >>> 0;
    const header = Buffer.alloc(30);
    header.writeUInt32LE(0x04034b50, 0);
    header.writeUInt16LE(20, 4); header.writeUInt16LE(0x800, 6); header.writeUInt16LE(8, 8);
    header.writeUInt16LE(33, 12); // DOS date: 1980-01-01, deterministic archive metadata.
    header.writeUInt32LE(crc, 14); header.writeUInt32LE(compressed.length, 18);
    header.writeUInt32LE(file.data.length, 22); header.writeUInt16LE(name.length, 26);
    local.push(header, name, compressed);
    const entry = Buffer.alloc(46);
    entry.writeUInt32LE(0x02014b50, 0); entry.writeUInt16LE(20, 4); entry.writeUInt16LE(20, 6);
    entry.writeUInt16LE(0x800, 8); entry.writeUInt16LE(8, 10); entry.writeUInt16LE(33, 14);
    entry.writeUInt32LE(crc, 16); entry.writeUInt32LE(compressed.length, 20);
    entry.writeUInt32LE(file.data.length, 24); entry.writeUInt16LE(name.length, 28);
    entry.writeUInt32LE(offset, 42);
    central.push(entry, name);
    offset += header.length + name.length + compressed.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(files.length, 8); end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, directory, end]);
}
