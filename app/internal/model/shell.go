package model

type ShellState struct {
	Cwd              string `json:"cwd"`
	Revision         string `json:"revision"`
	AltScreen        bool   `json:"altScreen"`
	PendingDirectory string `json:"pendingDirectory"`
}

type CommandEntry struct {
	Sequence string `json:"sequence"`
	Time     string `json:"time"`
	Command  string `json:"command"`
}
