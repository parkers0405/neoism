// Preserve the real child's automation pipes while reporting native exits that
// otherwise leave the automation client's pending request unresolved.
export {};
const command = [process.execPath, ...process.argv.slice(2)];
const child = Bun.spawn(process.env.NEOISM_BLOCK_E2E_GDB === "1" ? ["gdb", "--batch", "--return-child-result", "-ex", "set pagination off", "-ex", `set logging file ${process.env.NEOISM_GPUIX_STATE_PATH}.gdb.log`, "-ex", "set logging redirect on", "-ex", "set logging enabled on", "-ex", "handle SIGUSR1 nostop noprint pass", "-ex", "handle SIGUSR2 nostop noprint pass", "-ex", "handle SIGPWR nostop noprint pass", "-ex", "run", "-ex", "thread apply all bt 12", "--args", ...command] : command, { stdin: "inherit", stdout: "inherit", stderr: "inherit", env: process.env });
process.on("SIGTERM", () => child.kill("SIGTERM"));
process.on("SIGINT", () => child.kill("SIGINT"));
const status = await child.exited;
console.error(`Native e2e child exited: code=${status} signal=${child.signalCode ?? "none"}`);
process.exit(status);