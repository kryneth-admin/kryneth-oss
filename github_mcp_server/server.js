import express from 'express';
import { spawn } from 'child_process';
import { SSEServerTransport } from '@modelcontextprotocol/sdk/server/sse.js';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function loadGithubToken() {
    let token = process.env.GITHUB_PERSONAL_ACCESS_TOKEN;
    if (!token) {
        try {
            const envPath = path.resolve(__dirname, '../.env');
            if (fs.existsSync(envPath)) {
                const envContent = fs.readFileSync(envPath, 'utf8');
                const match = envContent.match(/^GITHUB_PERSONAL_ACCESS_TOKEN=(.*)$/m);
                if (match) {
                    token = match[1].trim();
                    if ((token.startsWith('"') && token.endsWith('"')) || 
                        (token.startsWith("'") && token.endsWith("'"))) {
                        token = token.slice(1, -1);
                    }
                }
            }
        } catch (err) {
            console.error("Failed to read ../.env file:", err);
        }
    }
    return token;
}

const app = express();
app.use(express.json());

const sessions = new Map();
const pendingRequests = new Map();
let messageIdCounter = 1;
let globalChild = null;

function spawnGlobalChild() {
    const githubToken = loadGithubToken();
    if (!githubToken) {
        console.error("Warning: GITHUB_PERSONAL_ACCESS_TOKEN is not configured in ../.env or process.env!");
    }
    
    console.log("Spawning persistent global GitHub MCP server process...");
    globalChild = spawn('npx', ['-y', '@modelcontextprotocol/server-github'], {
        shell: true,
        env: {
            ...process.env,
            GITHUB_PERSONAL_ACCESS_TOKEN: githubToken
        }
    });

    let globalBuffer = "";
    globalChild.stdout.on('data', (chunk) => {
        globalBuffer += chunk.toString();
        const lines = globalBuffer.split('\n');
        globalBuffer = lines.pop();
        for (const line of lines) {
            if (line.trim()) {
                try {
                    const message = JSON.parse(line);
                    const internalId = message.id;
                    
                    if (pendingRequests.has(internalId)) {
                        const { res, originalId } = pendingRequests.get(internalId);
                        pendingRequests.delete(internalId);
                        
                        // Restore the original ID
                        message.id = originalId;
                        console.log(`[Global Stdout -> Client POST] Resolving request ID ${originalId} (internal ${internalId})`);
                        res.json(message);
                    } else {
                        console.log("[Global Stdout] Message not matching any pending request:", JSON.stringify(message));
                    }
                } catch (e) {
                    console.error("Failed to parse global stdout line as JSON:", line, e);
                }
            }
        }
    });

    globalChild.stderr.on('data', (chunk) => {
        console.error("[Global Child Stderr]:", chunk.toString());
    });

    globalChild.on('close', (code) => {
        console.log(`Global child process exited with code ${code}. Restarting in 1s...`);
        for (const [internalId, { res, originalId }] of pendingRequests.entries()) {
            res.status(500).json({
                jsonrpc: "2.0",
                id: originalId,
                error: { code: -32603, message: "GitHub MCP Server exited unexpectedly" }
            });
        }
        pendingRequests.clear();
        setTimeout(spawnGlobalChild, 1000);
    });

    globalChild.on('error', (err) => {
        console.error("Global child process error:", err);
    });
}

function sendToGlobalChild(message, res) {
    const originalId = message.id;
    const internalId = messageIdCounter++;
    
    pendingRequests.set(internalId, { res, originalId });
    
    // Clone and rewrite the message ID to our internal unique ID
    const clonedMessage = { ...message, id: internalId };
    
    if (globalChild && globalChild.stdin.writable) {
        console.log(`[POST -> Global Stdin] Sending message ID ${internalId} (original ${originalId}):`, JSON.stringify(clonedMessage));
        globalChild.stdin.write(JSON.stringify(clonedMessage) + "\n");
    } else {
        console.error("Global child process stdin is not writable!");
        res.status(500).json({
            jsonrpc: "2.0",
            id: originalId,
            error: { code: -32603, message: "GitHub MCP Server is not running" }
        });
        pendingRequests.delete(internalId);
    }
}

// Start persistent global child process
spawnGlobalChild();

app.get('/sse', async (req, res) => {
    console.log("New SSE client connecting to /sse...");
    const transport = new SSEServerTransport('/messages', res);
    await transport.start();

    const sessionId = transport.sessionId;
    console.log(`SSE transport started with Session ID: ${sessionId}`);

    const githubToken = loadGithubToken();
    const child = spawn('npx', ['-y', '@modelcontextprotocol/server-github'], {
        shell: true,
        env: {
            ...process.env,
            GITHUB_PERSONAL_ACCESS_TOKEN: githubToken
        }
    });

    sessions.set(sessionId, { transport, child });

    let buffer = "";
    child.stdout.on('data', (chunk) => {
        buffer += chunk.toString();
        const lines = buffer.split('\n');
        buffer = lines.pop();
        for (const line of lines) {
            if (line.trim()) {
                try {
                    const message = JSON.parse(line);
                    console.log(`[Session Child Stdout -> Client] Session ${sessionId}:`, JSON.stringify(message));
                    transport.send(message).catch(err => {
                        console.error(`Failed to send message to SSE client: ${err.message}`);
                    });
                } catch (e) {
                    console.error(`Failed to parse session stdout line as JSON: "${line}"`, e);
                }
            }
        }
    });

    child.stderr.on('data', (chunk) => {
        console.error(`[Session Child Stderr] Session ${sessionId}:`, chunk.toString());
    });

    child.on('close', (code) => {
        console.log(`Session child process for session ${sessionId} exited with code ${code}`);
        if (sessions.has(sessionId)) {
            sessions.delete(sessionId);
            transport.close().catch(() => {});
        }
    });

    child.on('error', (err) => {
        console.error(`Session child process error for session ${sessionId}:`, err);
    });

    transport.onmessage = (message) => {
        console.log(`[Session Client POST -> Child Stdin] Session ${sessionId}:`, JSON.stringify(message));
        if (child.stdin.writable) {
            child.stdin.write(JSON.stringify(message) + '\n');
        }
    };

    transport.onclose = () => {
        console.log(`Session SSE client disconnected for session ${sessionId}`);
        if (sessions.has(sessionId)) {
            const session = sessions.get(sessionId);
            sessions.delete(sessionId);
            try {
                session.child.kill();
            } catch (e) {
                console.error(`Failed to kill session child process:`, e);
            }
        }
    };
});

async function handlePost(req, res) {
    const sessionId = req.query.sessionId;
    if (sessionId) {
        const session = sessions.get(sessionId);
        if (!session) {
            return res.status(404).send("Session not found");
        }
        try {
            await session.transport.handlePostMessage(req, res, req.body);
        } catch (err) {
            console.error(`Error handling session post message:`, err);
            if (!res.headersSent) {
                res.status(500).send(err.message);
            }
        }
    } else {
        sendToGlobalChild(req.body, res);
    }
}

app.post('/sse/messages', handlePost);
app.post('/messages', handlePost);

app.listen(3001, () => {
    console.log("GitHub MCP SSE Bridge Server running on http://localhost:3001");
});