# ADR 0002：使用 Seatbelt（sandbox-exec）而非容器
- 决策：每会话工作区可写、其余只读、默认断网的 SBPL 策略（改编自 openai/codex）；App 不启用 App Sandbox。
- 理由：零启动开销、无需额外运行时；Apple Containerization 需 Linux 镜像与内核、体积大、API 未稳定。
- 风险：sandbox-exec 标注为 deprecated；路径必须用 realpath；不能限制内存/CPU（用超时 + 进程终止兜底）。保留 Executor 抽象以便未来增加容器后端。
