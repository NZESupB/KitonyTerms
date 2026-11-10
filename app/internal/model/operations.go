package model

// 运维入口只接受已知类型和经校验的标识，不接受 shell 文本或 sudo 密码。
type OperationRequest struct {
	ID       string   `json:"id"`
	Kind     string   `json:"kind"`
	Resource string   `json:"resource"`
	Target   string   `json:"target"`
	Action   string   `json:"action"`
	PID      uint32   `json:"pid"`
	Lines    int      `json:"lines"`
	Targets  []string `json:"targets"`
}

type OperationError struct {
	Kind    string `json:"kind"`
	Message string `json:"message"`
}

func (e *OperationError) Error() string { return e.Message }

type OperationsCapabilities struct {
	Linux        bool `json:"linux"`
	Systemd      bool `json:"systemd"`
	PS           bool `json:"ps"`
	Proc         bool `json:"proc"`
	SS           bool `json:"ss"`
	ProcNet      bool `json:"proc_net"`
	DockerDaemon bool `json:"docker_daemon"`
	Compose      bool `json:"compose"`
}

type ServiceSummary struct {
	Name        string `json:"name"`
	LoadState   string `json:"loadState"`
	ActiveState string `json:"activeState"`
	SubState    string `json:"subState"`
	Description string `json:"description"`
}

type ProcessSummary struct {
	PID           uint32  `json:"pid"`
	PPID          uint32  `json:"ppid"`
	UID           uint32  `json:"uid"`
	State         string  `json:"state"`
	CPUPercent    float64 `json:"cpuPercent"`
	MemoryPercent float64 `json:"memoryPercent"`
	RSSKiB        string  `json:"rssKiB"`
	VSZKiB        string  `json:"vszKiB"`
	Elapsed       string  `json:"elapsed"`
	Command       string  `json:"command"`
}

type NetworkConnection struct {
	Protocol       string `json:"protocol"`
	State          string `json:"state"`
	Local          string `json:"local"`
	Peer           string `json:"peer"`
	Owner          string `json:"owner"`
	OwnerAvailable bool   `json:"ownerAvailable"`
}

// Docker 不同资源共用标识及展示字段，资源类型由结果 kind 确定。
type DockerItem struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Status      string `json:"status"`
	State       string `json:"state"`
	Image       string `json:"image"`
	Repository  string `json:"repository"`
	Tag         string `json:"tag"`
	Size        string `json:"size"`
	Created     string `json:"created"`
	Driver      string `json:"driver"`
	Scope       string `json:"scope"`
	ConfigFiles string `json:"configFiles"`
}

type DockerRuntime struct {
	ID               string   `json:"id"`
	CPUPercent       *float64 `json:"cpuPercent"`
	MemoryPercent    *float64 `json:"memoryPercent"`
	MemoryUsedBytes  string   `json:"memoryUsedBytes"`
	MemoryLimitBytes string   `json:"memoryLimitBytes"`
}

type DockerStorage struct {
	ID         string `json:"id"`
	ImageID    string `json:"imageId"`
	SizeRW     string `json:"sizeRW"`
	SizeRootFS string `json:"sizeRootFS"`
	Size       string `json:"size"`
}

type OperationReply struct {
	RequestID    string                  `json:"requestId"`
	Kind         string                  `json:"kind"`
	Capabilities *OperationsCapabilities `json:"capabilities,omitempty"`
	Services     []ServiceSummary        `json:"services"`
	Processes    []ProcessSummary        `json:"processes"`
	Network      []NetworkConnection     `json:"network"`
	Docker       []DockerItem            `json:"docker"`
	Runtime      []DockerRuntime         `json:"runtime"`
	Storage      []DockerStorage         `json:"storage"`
	Title        string                  `json:"title"`
	Output       string                  `json:"output"`
	Error        *OperationError         `json:"error,omitempty"`
}

type OperationChallenge struct {
	OperationID string `json:"operationId"`
	ID          string `json:"id"`
	Message     string `json:"message"`
}

type OperationAnswer struct {
	SessionID   string `json:"sessionId"`
	Generation  string `json:"generation"`
	OperationID string `json:"operationId"`
	ChallengeID string `json:"challengeId"`
	Password    string `json:"password"`
	Cancel      bool   `json:"cancel"`
}
