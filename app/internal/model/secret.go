package model

type SecretState struct {
	Present         bool `json:"present"`
	Available       bool `json:"available"`
	AccountPassword bool `json:"accountPassword"`
}

type SecretReveal struct {
	Value   string `json:"value"`
	Failure string `json:"failure"`
}
