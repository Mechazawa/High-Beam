import { resolve } from "path";
import fs from "fs";

const { readdir } = fs.promises;

export async function* getFiles (dir, skipMacFiles = true) {
  const dirents = await readdir(dir, { withFileTypes: true });
  for (const dirent of dirents) {
    const skip = skipMacFiles && /\.\w+$/.test(dirent.name);
    const res = resolve(dir, dirent.name);

    if (!skip && dirent.isDirectory()) {
      yield* getFiles(res);
    } else {
      yield res;
    }
  }
}