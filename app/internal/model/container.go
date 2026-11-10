package model

// ExecID 独立于运维请求与宿主会话，所有后续调用必须同时匹配连接代次。
type ContainerTerminalRequest struct {
	ExecID    string `json:"execId"`
	Container string `json:"container"`
	Cols      int    `json:"cols"`
	Rows      int    `json:"rows"`
}

type ContainerTerminalSnapshot struct {
	ExecID        string `json:"execId"`
	Container     string `json:"container"`
	Status        string `json:"status"`
	Error         string `json:"error"`
	InputSequence string `json:"inputSequence"`
}

type ContainerInputRequest struct {
	SessionID  string `json:"sessionId"`
	Generation string `json:"generation"`
	ExecID     string `json:"execId"`
	Sequence   string `json:"sequence"`
	Data       string `json:"data"`
}
