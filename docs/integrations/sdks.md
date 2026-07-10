# Drop-in SDK Client Integrations

Kryneth operates as a drop-in reverse proxy for popular AI provider SDKs. This means you do not need to install custom client libraries; simply redirect existing OpenAI or Anthropic clients to Kryneth's endpoint port.

---

## 1. Python SDKs

### OpenAI Python SDK
Update the client initialization to target the Kryneth gateway:

```python
import os
from openai import OpenAI

# Direct client connection routed through Kryneth
client = OpenAI(
    base_url="http://localhost:8080/v1",
    api_key=os.getenv("KRYNETH_API_KEY", "re_live_local_dev_123")
)

response = client.chat.completions.create(
    model="llama-3.3-70b-versatile",
    messages=[{"role": "user", "content": "Explain agent safety."}]
)
print(response.choices[0].message.content)
```

### LangChain Integration
```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    base_url="http://localhost:8080/v1",
    api_key="re_live_local_dev_123",
    model="llama-3.3-70b-versatile"
)

response = llm.invoke("Hello, Kryneth!")
print(response.content)
```

---

## 2. JavaScript / Node.js SDK

Modify the `baseURL` parameter:

```javascript
import OpenAI from 'openai';

const openai = new OpenAI({
  baseURL: 'http://localhost:8080/v1',
  apiKey: process.env.KRYNETH_API_KEY || 're_live_local_dev_123'
});

const completion = await openai.chat.completions.create({
  model: 'llama-3.3-70b-versatile',
  messages: [{ role: 'user', content: 'Execute local check.' }],
});
console.log(completion.choices[0].message.content);
```

---

## 3. Rust Integration (`async-openai`)

```rust
use async_openai::{Client, config::OpenAIConfig};

let config = OpenAIConfig::new()
    .with_api_base("http://localhost:8080/v1")
    .with_api_key("re_live_local_dev_123");
    
let client = Client::with_config(config);
```
