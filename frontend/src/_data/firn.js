import { readFileSync } from "node:fs";

const { version, repository } = JSON.parse(readFileSync("package.json", "utf8"));

export default { version, repository };
