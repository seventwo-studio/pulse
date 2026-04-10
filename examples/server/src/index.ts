import { Hono } from "hono";
import { createDatabase } from "./db";
import { repoRoutes } from "./routes/repo";
import { mainRoutes } from "./routes/main";
import { objectRoutes } from "./routes/objects";
import { workspaceRoutes } from "./routes/workspaces";
import { diffRoutes } from "./routes/diff";
import { fileRoutes } from "./routes/files";
import { syncRoutes } from "./routes/sync";

const dbPath = process.env.PULSE_DB ?? "pulse.db";
const port = Number(process.env.PORT ?? 3000);

const db = createDatabase(dbPath);
const app = new Hono();

app.route("/", repoRoutes(db));
app.route("/", mainRoutes(db));
app.route("/", objectRoutes(db));
app.route("/", workspaceRoutes(db));
app.route("/", diffRoutes(db));
app.route("/", fileRoutes(db));
app.route("/", syncRoutes(db));

console.log(`Pulse example server listening on http://localhost:${port}`);
console.log(`Database: ${dbPath}`);

export default {
  port,
  fetch: app.fetch,
};
