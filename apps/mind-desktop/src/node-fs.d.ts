declare module "node:fs" {
  interface BinaryFile {
    subarray(start: number, end: number): BinaryFile;
    toString(encoding: "utf8"): string;
  }

  export function readFileSync(path: URL, encoding: "utf8"): string;
  export function readFileSync(path: URL): BinaryFile;
}
