package model

type AppActivity struct {
	Sessions           int `json:"sessions"`
	Transfers          int `json:"transfers"`
	Operations         int `json:"operations"`
	ContainerTerminals int `json:"containerTerminals"`
}

type QuitState struct {
	RequestID           string      `json:"requestId"`
	Preparing           bool        `json:"preparing"`
	Intent              string      `json:"intent"`
	Ready               bool        `json:"ready"`
	Committing          bool        `json:"committing"`
	Cancelling          bool        `json:"cancelling"`
	ProtectedOperations int         `json:"protectedOperations"`
	Activity            AppActivity `json:"activity"`
	Sharing             bool        `json:"sharing"`
	ExternalEdits       int         `json:"externalEdits"`
}
