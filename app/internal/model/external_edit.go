package model

type ExternalEdit struct {
	ID         string `json:"id"`
	SessionID  string `json:"sessionId"`
	Generation string `json:"generation"`
	RemotePath string `json:"remotePath"`
	LocalPath  string `json:"localPath"`
	Status     string `json:"status"`
	AutoUpload bool   `json:"autoUpload"`
	Dirty      bool   `json:"dirty"`
	Error      string `json:"error"`
}
