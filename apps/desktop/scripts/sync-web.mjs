// Resources/web (ECharts dashboard / chart / ER renderers) → public/web, served same-origin for the iframes.
import { cpSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
const here = dirname(fileURLToPath(import.meta.url));
const dest = join(here, "../public/web");
rmSync(dest, { recursive: true, force: true });
cpSync(join(here, "../../../Resources/web"), dest, { recursive: true });
console.log("synced Resources/web → public/web");
