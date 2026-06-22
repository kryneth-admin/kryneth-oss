Output agent_loop_blocked.gif

Set Theme { "background": "#1e1e1e", "foreground": "#e0e0e0" }
Set FontFamily "JetBrains Mono"
Set FontSize 18
Set Width 1200
Set Height 800
Set Padding 40

Hide
Type "mock_curl() {"
Enter
Type "  sleep 0.2"
Enter
Type "  cat << 'EOF'"
Enter
Type "HTTP/1.1 403 Forbidden"
Enter
Type "Content-Type: application/json"
Enter
Type "x-kryneth-latency: 0.6ms"
Enter
Type "x-kryneth-version: 1.0.0-oss"
Enter
Type ""
Enter
Type "{"
Enter
Type '  "error": "Agent infinite loop detected for identical tool signature. Request blocked."'
Enter
Type "}"
Enter
Type "EOF"
Enter
Type "}"
Enter
Type "alias curl=mock_curl"
Enter
Type "clear"
Enter
Show

Type "echo '# Kryneth Guardian: Detecting an Agentic Runaway Loop'"
Enter
Sleep 1s
Type "echo '# An agent got stuck calling the same tool repeatedly. Watch Kryneth block it.'"
Enter
Sleep 1.5s

Type "curl -is -X POST http://localhost:8080/v1/chat/completions \"
Enter
Type "  -H 'Authorization: Bearer sk-agent-123' \"
Enter
Type "  -d '{"
Enter
Type '    "model": "gpt-4-turbo",'
Enter
Type '    "messages": ['
Enter
Type '      {"role": "assistant", "tool_calls": [{"function": {"name": "fetch_data", "arguments": "{\"id\": 1}"}}]}'
Enter
Type '    ]'
Enter
Type "  }'"
Enter

Sleep 1.5s

Type "echo '# Processed in 0.6ms! Kryneth zero-allocation byte-scanning blocked the 6th identical call.'"
Enter
Sleep 4s
