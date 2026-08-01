import { readFile, writeFile } from 'node:fs/promises';

const [localPath, agentPath, outputPath] = process.argv.slice(2);
if (!localPath || !agentPath || !outputPath) {
  console.error(
    'Usage: merge-openapi.mjs <local.json> <agent.json> <output.json>',
  );
  process.exit(1);
}

const local = JSON.parse(await readFile(localPath, 'utf8'));
const agent = JSON.parse(await readFile(agentPath, 'utf8'));

const merged = { ...local };

if (agent.paths) {
  merged.paths = { ...local.paths, ...agent.paths };
}

if (agent.components?.schemas) {
  merged.components ??= {};
  merged.components.schemas ??= {};
  for (const [name, schema] of Object.entries(agent.components.schemas)) {
    if (name in merged.components.schemas) {
      console.error(`schema name collision: ${name}`);
      process.exit(1);
    }
    merged.components.schemas[name] = schema;
  }
}

await writeFile(outputPath, JSON.stringify(merged, null, 2));
console.log(`merged OpenAPI spec written to ${outputPath}`);
