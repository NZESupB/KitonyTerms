package model

type UpdateBuild struct {
	Channel string `json:"channel"`
	Version string `json:"version"`
	Commit  string `json:"commit"`
	Build   string `json:"build"`
	Attempt string `json:"attempt"`
	Target  string `json:"target"`
}

type UpdateProxy struct {
	Kind     string `json:"kind"`
	Host     string `json:"host,omitempty"`
	Port     int    `json:"port,omitempty"`
	Username string `json:"username,omitempty"`
}

type UpdatePreferences struct {
	AutoCheck    bool        `json:"auto_check"`
	AutoDownload bool        `json:"auto_download"`
	Proxy        UpdateProxy `json:"proxy"`
}

type SaveUpdatePreferencesRequest struct {
	ExpectedRevision string            `json:"expectedRevision"`
	Preferences      UpdatePreferences `json:"preferences"`
}

type UpdatePackage struct {
	Name   string `json:"name"`
	Size   string `json:"size"`
	SHA256 string `json:"sha256"`
	Kind   string `json:"kind"`
}

type UpdateAvailable struct {
	Info           UpdateBuild   `json:"info"`
	Package        UpdatePackage `json:"package"`
	AssetID        string        `json:"assetId"`
	ReleaseID      string        `json:"releaseId"`
	Tag            string        `json:"tag"`
	Notes          string        `json:"notes"`
	NotesTruncated bool          `json:"notesTruncated"`
	PublishedAt    string        `json:"publishedAt"`
}

type UpdateFailure struct {
	Kind    string `json:"kind"`
	Stage   string `json:"stage"`
	Status  int    `json:"status"`
	RetryAt string `json:"retryAt"`
}

type UpdateSnapshot struct {
	Sequence           string            `json:"sequence"`
	Phase              string            `json:"phase"`
	Current            UpdateBuild       `json:"current"`
	Preferences        UpdatePreferences `json:"preferences"`
	PreferenceRevision string            `json:"preferenceRevision"`
	Available          *UpdateAvailable  `json:"available"`
	Received           string            `json:"received"`
	Speed              string            `json:"speed"`
	LastCheck          string            `json:"lastCheck"`
	NextCheck          string            `json:"nextCheck"`
	Failure            *UpdateFailure    `json:"failure"`
	Notice             string            `json:"notice"`
	DisabledReason     string            `json:"disabledReason"`
	Busy               bool              `json:"busy"`
	InstallAvailable   bool              `json:"installAvailable"`
	RequiresElevation  bool              `json:"requiresElevation"`
}
