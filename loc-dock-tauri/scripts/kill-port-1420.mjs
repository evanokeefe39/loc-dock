// ponytail: kills stale Vite process on port 1420 before dev starts.
// Avoids JSON→npm→cmd escaping hell for `for /f %a in ('netstat...')`
import { execSync } from 'child_process';

try {
  const out = execSync('netstat -ano', { encoding: 'utf8' });
  for (const line of out.split('\n')) {
    if (line.includes(':1420') && line.includes('LISTENING')) {
      const pid = line.trim().split(/\s+/).pop();
      console.log(`[kill-port-1420] Killing PID ${pid} holding port 1420`);
      execSync(`taskkill /F /PID ${pid}`, { stdio: 'ignore' });
    }
  }
} catch {
  // no stale process → ok
}
