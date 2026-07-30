import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const request = JSON.parse(fs.readFileSync(0, "utf8"));
const entry = path.join(
  request.module_dir,
  "@xenova",
  "transformers",
  "src",
  "transformers.js",
);
const { env, pipeline } = await import(pathToFileURL(entry).href);

env.cacheDir = request.cache_dir;
env.allowLocalModels = true;
env.allowRemoteModels = request.allow_remote === true;

const extractor = await pipeline("feature-extraction", request.model, {
  cache_dir: request.cache_dir,
});
const vectors = [];
for (const text of request.texts) {
  const output = await extractor(`${request.prefix}${text}`, {
    pooling: "mean",
    normalize: true,
  });
  vectors.push(output.tolist()[0]);
}
process.stdout.write(JSON.stringify({ vectors }));
