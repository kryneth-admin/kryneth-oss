Output llm_hotswap.gif

Set Theme { "background": "#1e1e1e", "foreground": "#e0e0e0" }
Set FontFamily "JetBrains Mono"
Set FontSize 18
Set Width 1200
Set Height 800
Set Padding 40

Hide
Type "mock_curl() {"
Enter
Type "  sleep 0.3"
Enter
Type "  cat << 'EOF'"
Enter
Type "HTTP/1.1 200 OK"
Enter
Type "Content-Type: application/json"
Enter
Type "x-kryneth-latency: 1.2ms"
Enter
Type "x-kryneth-circuit-breaker: tripped (openai -> anthropic)"
Enter
Type "x-kryneth-hot-swapped: true"
Enter
Type ""
Enter
Type "{"
Enter
Type '  "id": "chatcmpl-fallback",'
Enter
Type '  "object": "chat.completion",'
Enter
Type '  "created": 1718000000,'
Enter
Type '  "model": "claude-3-5-sonnet-20240620",'
Enter
Type '  "choices": ['
Enter
Type "    {"
Enter
Type '      "index": 0,'
Enter
Type '      "message": {'
Enter
Type '        "role": "assistant",'
Enter
Type '        "content": "I have successfully processed your request via the fallback provider."'
Enter
Type "      },"
Enter
Type '      "finish_reason": "stop"'
Enter
Type "    }"
Enter
Type "  ]"
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

Type "echo '# Kryneth Router: LLM Provider Rate Limit Auto Hot-Swap'"
Enter
Sleep 1s
Type "echo '# OpenAI API hits a 429 Rate Limit. Kryneth instantly falls back to Anthropic.'"
Enter
Sleep 1.5s

Type "curl -is -X POST http://localhost:8080/v1/chat/completions \"
Enter
Type "  -H 'Authorization: Bearer sk-tenant-abc' \"
Enter
Type "  -d '{"
Enter
Type '    "model": "my-prod-route",'
Enter
Type '    "messages": [{"role": "user", "content": "Summarize the latest financial report."}]'
Enter
Type "  }'"
Enter

Sleep 1.5s

Type "echo '# Kryneth Circuit Breaker Tripped! Hot-swapped to Anthropic schema transparently.'"
Enter
Sleep 1s
Type "echo '# Processed in 1.2ms (Zero-Copy Translation).'"
Enter
Sleep 4s
