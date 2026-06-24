Output docs/quickstart_demo.gif
Require docker
Require curl
Set Margin 20
Set MarginFill "#000000"
Set BorderRadius 8
Set WindowBar "Rings"
Set Theme {"background": "#0D0D0D", "foreground": "#FFFFFF", "cursor": "#FFFFFF", "black": "#0D0D0D", "white": "#FFFFFF", "gray": "#888888", "green": "#FFFFFF", "blue": "#FFFFFF"}
Set Width 1200
Set Height 650
Set TypingSpeed 40ms

Hide
Type "clear"
Enter
Show
Sleep 1s
Type "# 1. Spin up Kryneth Gateway via Docker"
Sleep 500ms
Enter
Type "docker run -d -p 8080:8080 --name kryneth-core crossroot/kryneth-gateway:latest"
Enter
Sleep 3s
Type "clear"
Enter
Sleep 500ms
Type "# 2. Drop-in OpenAI Compatible Request"
Sleep 500ms
Enter
Type "curl -s -X POST http://localhost:8080/v1/chat/completions \"
Enter
Type "  -H 'Content-Type: application/json' \"
Enter
Type "  -H 'Authorization: Bearer sk-local-dev-key' \"
Enter
Type "  -d '{"
Enter
Type '    "model": "claude-3-opus",'
Enter
Type '    "messages": [{"role": "user", "content": "Initialize core systems."}]'
Enter
Type "  }' | jq"
Enter
Sleep 3s
Type "..."
Sleep 1s
Hide
Type "clear"
Enter
Type "cat << 'EOF'"
Enter
Type "{"
Enter
Type '  "id": "chatcmpl-kryneth-1234",'
Enter
Type '  "object": "chat.completion",'
Enter
Type '  "created": 1718881234,'
Enter
Type '  "model": "claude-3-opus",'
Enter
Type '  "choices": ['
Enter
Type '    {'
Enter
Type '      "index": 0,'
Enter
Type '      "message": {'
Enter
Type '        "role": "assistant",'
Enter
Type '        "content": "Core systems initialized. Multi-LLM routing active. TTFV optimal."'
Enter
Type '      },'
Enter
Type '      "finish_reason": "stop"'
Enter
Type '    }'
Enter
Type '  ],'
Enter
Type '  "usage": {'
Enter
Type '    "prompt_tokens": 12,'
Enter
Type '    "completion_tokens": 15,'
Enter
Type '    "total_tokens": 27'
Enter
Type '  }'
Enter
Type "}"
Enter
Type "EOF"
Enter
Show
Sleep 4s
