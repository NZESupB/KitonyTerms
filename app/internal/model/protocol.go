package model

type Prompt struct {
	Text string `json:"text"`
	Echo bool   `json:"echo"`
}
type Challenge struct {
	ID          string   `json:"id"`
	Kind        string   `json:"kind"`
	Host        string   `json:"host"`
	Port        int      `json:"port"`
	Fingerprint string   `json:"fingerprint"`
	Expected    string   `json:"expected"`
	Prompts     []Prompt `json:"prompts"`
	Message     string   `json:"message"`
}
type SessionSnapshot struct {
	ID         string     `json:"id"`
	Generation string     `json:"generation"`
	Title      string     `json:"title"`
	Status     string     `json:"status"`
	Error      string     `json:"error"`
	Challenge  *Challenge `json:"challenge,omitempty"`
}
type ChallengeAnswer struct {
	SessionID   string   `json:"sessionId"`
	Generation  string   `json:"generation"`
	ChallengeID string   `json:"challengeId"`
	Answers     []string `json:"answers"`
	Trust       bool     `json:"trust"`
}
type InputRequest struct {
	SessionID  string `json:"sessionId"`
	Generation string `json:"generation"`
	Sequence   string `json:"sequence"`
	Data       string `json:"data"`
}
type TerminalState struct {
	InputSequence string `json:"inputSequence"`
}
type TerminalChunk struct {
	Sequence string `json:"sequence"`
	Data     string `json:"data"`
}
type TerminalBatch struct {
	Chunks       []TerminalChunk `json:"chunks"`
	NextSequence string          `json:"nextSequence"`
	Closed       bool            `json:"closed"`
	Error        string          `json:"error"`
	Reset        bool            `json:"reset"`
}
type FileEntry struct {
	Name       string `json:"name"`
	Path       string `json:"path"`
	IsDir      bool   `json:"isDir"`
	IsSymlink  bool   `json:"isSymlink"`
	Size       string `json:"size"`
	Modified   string `json:"modified"`
	Mode       string `json:"mode"`
	OwnerGroup string `json:"ownerGroup"`
}
type Directory struct {
	Path    string      `json:"path"`
	Entries []FileEntry `json:"entries"`
}
type TextFile struct {
	Path     string `json:"path"`
	Content  string `json:"content"`
	Revision string `json:"revision"`
}
type OperationResult struct {
	Output string `json:"output"`
}
type AppInfo struct {
	Platform      string   `json:"platform"`
	Mobile        bool     `json:"mobile"`
	DataDirectory string   `json:"dataDirectory"`
	Capabilities  []string `json:"capabilities"`
	Limitations   []string `json:"limitations"`
}

type TransferSummary struct {
	Files int    `json:"files"`
	Bytes string `json:"bytes"`
}

// 字节计数使用十进制字符串，避免前端整数精度丢失。
type TransferProgress struct {
	ID         string `json:"id"`
	SessionID  string `json:"sessionId"`
	Generation string `json:"generation"`
	Direction  string `json:"direction"`
	Status     string `json:"status"`
	Current    string `json:"current"`
	Files      int    `json:"files"`
	TotalFiles int    `json:"totalFiles"`
	Bytes      string `json:"bytes"`
	TotalBytes string `json:"totalBytes"`
	Error      string `json:"error"`
}
