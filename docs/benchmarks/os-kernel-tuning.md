# Operating System & Kernel Tuning Guide

When deploying Kryneth Gateway or conducting high-intensity load tests locally (specifically using Windows Subsystem for Linux (WSL2) or raw Linux host environments), OS-level resource limits can become bottlenecks before the gateway pipeline itself saturates. 

This guide outlines the critical kernel and resource tuning configurations required to achieve optimal throughput.

---

## 1. WSL2 Resource Capping via `.wslconfig`

By default, WSL2 allocates up to 50% of the host Windows machine's physical memory and all available CPU cores. Under massive network loopback I/O, the WSL VM can experience severe memory leaks due to cached file systems or excessive virtual networking overhead.

To prevent host system instability and ensure deterministic performance benchmarks, resource boundaries must be configured.

### Recommended Host Configuration

Create or edit the global `.wslconfig` file in the Windows user profile path:
`C:\Users\<Your-Username>\.wslconfig` (or `%USERPROFILE%\.wslconfig`)

```ini
[wsl2]
# Restrict WSL2 memory allocation to prevent host Windows memory starvation
memory=16GB

# Allocate CPU cores (recommend 50% to 75% of available logical processors)
processors=8

# Enable localhost forwarding to bind gateway ports (8080) to Windows host loopback
localhostForwarding=true

# Conserve resources by disabling virtual graphic interfaces
guiApplications=false

# Gradually release cached virtual machine memory back to Windows
autoMemoryReclaim=gradual
```

### Applying WSL2 Configuration

To apply these boundaries, open PowerShell on the Windows host and execute:
```powershell
wsl --shutdown
```
Upon running your next command in the WSL terminal (e.g. `make setup`), the environment will boot using the newly provisioned limits.

---

## 2. Linux Page Cache Purging (`drop_caches`)

Under heavy simulated client load, the Linux kernel aggressively caches file systems, inodes, and directory entries (dentries) to optimize disk I/O. In high-concurrency environments like our load testing pipeline, these page caches can grow to consume all allocated RAM, triggering the Out-Of-Memory (OOM) killer.

### Manual Cache Eviction

To reclaim memory immediately without stopping services, write to the `/proc/sys/vm/drop_caches` interface.

> [!CAUTION]
> Writing to `drop_caches` is a clean operation but can temporarily increase disk read latency as the OS must re-read metadata from disk. Always run `sync` first to ensure all dirty pages in memory are flushed to disk.

#### I. Flush Page Cache Only
```bash
sync; sudo sh -c 'echo 1 > /proc/sys/vm/drop_caches'
```

#### II. Flush Dentries and Inodes Only
```bash
sync; sudo sh -c 'echo 2 > /proc/sys/vm/drop_caches'
```

#### III. Flush Page Cache, Dentries, and Inodes (Complete Purge)
```bash
sync; sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
```

---

## 3. High-Throughput Linux Socket and File Descriptor Tuning

To handle more than 10,000 concurrent socket connections without encountering "Too many open files" or port exhaustion errors, tune the network stack parameters in `/etc/sysctl.conf`.

### sysctl.conf Performance Tunings

Append the following lines to `/etc/sysctl.conf` to optimize loopback traffic:

```ini
# Enable fast recycling of sockets in TIME_WAIT status for local loopback
net.ipv4.tcp_tw_reuse = 1

# Increase the maximum number of open file descriptors system-wide
fs.file-max = 2097152

# Increase the maximum queue length of connections awaiting accept (backlog)
net.core.somaxconn = 65535

# Optimize TCP window buffers for high throughput
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 65536 16777216
```

Apply these settings immediately without rebooting:
```bash
sudo sysctl -p
```
