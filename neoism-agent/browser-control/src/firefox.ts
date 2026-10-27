process.env.NEOISM_BROWSER_ENGINE="firefox"
process.env.NEOISM_BROWSER_ENTRY="firefox"
const {main}=await import("./mcp.ts")
await main()
