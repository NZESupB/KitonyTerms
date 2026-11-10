package model

import (
	"fmt"
	"math"
	"net"
	"strings"
)

type AuthMethod struct {
	Kind    string `json:"kind" toml:"kind"`
	KeyPath string `json:"key_path,omitempty" toml:"key_path,omitempty"`
}
type Proxy struct {
	Kind     string `json:"kind" toml:"kind"`
	Host     string `json:"host,omitempty" toml:"host,omitempty"`
	Port     int    `json:"port,omitempty" toml:"port,omitempty"`
	Username string `json:"username,omitempty" toml:"username,omitempty"`
}
type Profile struct {
	Name         string       `json:"name" toml:"name"`
	Group        string       `json:"group,omitempty" toml:"group,omitempty"`
	Host         string       `json:"host" toml:"host"`
	Port         int          `json:"port" toml:"port"`
	User         string       `json:"user" toml:"user"`
	Auth         []AuthMethod `json:"auth" toml:"auth"`
	VaultID      string       `json:"vault_id,omitempty" toml:"vault_id,omitempty"`
	ProxyJump    string       `json:"proxy_jump,omitempty" toml:"proxy_jump,omitempty"`
	Proxy        Proxy        `json:"proxy" toml:"proxy,omitempty"`
	ForwardAgent bool         `json:"forward_agent" toml:"forward_agent"`
}

func (p Profile) SecretID() string {
	if p.VaultID != "" {
		return p.VaultID
	}
	return fmt.Sprintf("%s@%s:%d", p.User, p.Host, p.Port)
}
func (p *Profile) Normalize() error {
	p.Name = strings.TrimSpace(p.Name)
	p.Host = strings.TrimSpace(p.Host)
	p.User = strings.TrimSpace(p.User)
	if p.Port == 0 {
		p.Port = 22
	}
	if len(p.Auth) == 0 {
		p.Auth = []AuthMethod{{Kind: "password"}}
	}
	if p.Proxy.Kind == "" {
		p.Proxy.Kind = "direct"
	}
	if p.Name == "" || len(p.Name) > 256 || p.Host == "" || len(p.Host) > 1024 || p.User == "" || len(p.User) > 256 || p.Port < 1 || p.Port > 65535 {
		return fmt.Errorf("连接名称、主机、用户或端口无效")
	}
	if strings.ContainsAny(p.Name+p.Host+p.User, "\x00\r\n") || strings.ContainsAny(p.Host, " /\\") {
		return fmt.Errorf("连接字段包含无效字符")
	}
	for _, a := range p.Auth {
		switch a.Kind {
		case "password", "keyboard_interactive", "agent":
		case "public_key":
			if a.KeyPath == "" {
				return fmt.Errorf("私钥路径不能为空")
			}
		default:
			return fmt.Errorf("未知认证方式: %s", a.Kind)
		}
	}
	switch p.Proxy.Kind {
	case "direct", "system":
	case "http", "socks5":
		if p.Proxy.Host == "" || p.Proxy.Port < 1 || p.Proxy.Port > 65535 {
			return fmt.Errorf("代理地址无效")
		}
	default:
		return fmt.Errorf("未知代理类型")
	}
	if p.ProxyJump != "" && strings.ContainsAny(p.ProxyJump, ",\r\n\x00 ") {
		return fmt.Errorf("仅支持单跳 ProxyJump")
	}
	return nil
}
func (p Profile) Address() string { return net.JoinHostPort(p.Host, fmt.Sprint(p.Port)) }

type Editor struct {
	Name    string `json:"name" toml:"name"`
	Command string `json:"command" toml:"command"`
}
type Settings struct {
	Language          string   `json:"language" toml:"language"`
	FontFamily        string   `json:"font_family" toml:"font_family"`
	FontSize          float64  `json:"font_size" toml:"font_size"`
	UIFontFamily      string   `json:"ui_font_family,omitempty" toml:"ui_font_family,omitempty"`
	UIFontScale       float64  `json:"ui_font_scale" toml:"ui_font_scale"`
	Theme             string   `json:"theme" toml:"theme"`
	AccentColor       string   `json:"accent_color" toml:"accent_color"`
	ScrollbackLines   int      `json:"scrollback_lines" toml:"scrollback_lines"`
	CursorStyle       string   `json:"cursor_style" toml:"cursor_style"`
	UseSSHConfig      bool     `json:"use_ssh_config" toml:"use_ssh_config"`
	TriggerHighlights []string `json:"trigger_highlights" toml:"trigger_highlights"`
	DefaultEditor     string   `json:"default_editor,omitempty" toml:"default_editor,omitempty"`
	Editors           []Editor `json:"editors" toml:"editors,omitempty"`
	ShowLineNumbers   bool     `json:"show_line_numbers" toml:"show_line_numbers"`
	ShowTimestamps    bool     `json:"show_timestamps" toml:"show_timestamps"`
	TerminalWrap      bool     `json:"terminal_wrap" toml:"terminal_wrap"`
	SFTPAutoSync      bool     `json:"sftp_auto_sync" toml:"sftp_auto_sync"`
	SFTPColumnWidths  []int    `json:"sftp_column_widths,omitempty" toml:"sftp_column_widths,omitempty"`
}

func DefaultSettings() Settings {
	return Settings{Language: "chinese", FontFamily: "monospace", FontSize: 13, UIFontScale: 1, Theme: "default-dark", AccentColor: "#5aa7ff", ScrollbackLines: 10000, CursorStyle: "block", UseSSHConfig: true, TriggerHighlights: []string{"error", "failed", "warning", "panic"}, Editors: []Editor{}, TerminalWrap: true}
}
func (s Settings) Validate() error {
	if math.IsNaN(s.FontSize) || math.IsInf(s.FontSize, 0) || math.IsNaN(s.UIFontScale) || math.IsInf(s.UIFontScale, 0) || s.FontSize < 8 || s.FontSize > 48 || s.UIFontScale < .85 || s.UIFontScale > 1.4 || s.ScrollbackLines < 0 || s.ScrollbackLines > 100000 {
		return fmt.Errorf("字体或滚动缓冲设置超出范围")
	}
	switch s.Theme {
	case "system", "default-dark", "default-light", "dark", "light":
	default:
		return fmt.Errorf("主题无效")
	}
	if s.Language != "chinese" && s.Language != "english" {
		return fmt.Errorf("语言无效")
	}
	if s.CursorStyle != "block" && s.CursorStyle != "bar" && s.CursorStyle != "underline" {
		return fmt.Errorf("光标样式无效")
	}
	if len(s.AccentColor) != 7 || s.AccentColor[0] != '#' {
		return fmt.Errorf("强调色必须为六位十六进制颜色")
	}
	for _, ch := range s.AccentColor[1:] {
		if !(ch >= '0' && ch <= '9' || ch >= 'a' && ch <= 'f' || ch >= 'A' && ch <= 'F') {
			return fmt.Errorf("强调色必须为六位十六进制颜色")
		}
	}
	validText := func(value string, limit int) bool {
		return len(value) <= limit && !strings.ContainsAny(value, "\x00\r\n")
	}
	if strings.TrimSpace(s.FontFamily) == "" || !validText(s.FontFamily, 512) || !validText(s.UIFontFamily, 512) {
		return fmt.Errorf("字体名称无效或过长")
	}
	if len(s.TriggerHighlights) > 64 {
		return fmt.Errorf("最多设置64个提示关键字")
	}
	for _, value := range s.TriggerHighlights {
		if !validText(value, 256) {
			return fmt.Errorf("提示关键字无效或过长")
		}
	}
	if len(s.Editors) > 64 || !validText(s.DefaultEditor, 4096) {
		return fmt.Errorf("编辑器设置无效或过长")
	}
	commands := map[string]bool{}
	for _, editor := range s.Editors {
		command := strings.TrimSpace(editor.Command)
		if strings.TrimSpace(editor.Name) == "" || command == "" || !validText(editor.Name, 256) || !validText(editor.Command, 4096) || commands[command] {
			return fmt.Errorf("编辑器名称或命令无效、重复或过长")
		}
		commands[command] = true
	}
	return nil
}

type Config struct {
	Settings Settings  `json:"settings" toml:"settings"`
	Groups   []string  `json:"groups" toml:"groups"`
	Sessions []Profile `json:"sessions" toml:"sessions"`
}

func DefaultConfig() Config {
	return Config{Settings: DefaultSettings(), Groups: []string{}, Sessions: []Profile{}}
}

func (c *Config) Normalize() error {
	if c.Sessions == nil {
		c.Sessions = []Profile{}
	}
	if c.Groups == nil {
		c.Groups = []string{}
	}
	if c.Settings.Editors == nil {
		c.Settings.Editors = []Editor{}
	}
	if c.Settings.TriggerHighlights == nil {
		c.Settings.TriggerHighlights = []string{}
	}
	seen := map[string]bool{}
	for i := range c.Sessions {
		if err := c.Sessions[i].Normalize(); err != nil {
			return err
		}
		if seen[c.Sessions[i].Name] {
			return fmt.Errorf("连接名称重复")
		}
		seen[c.Sessions[i].Name] = true
	}
	return c.Settings.Validate()
}

type ConfigSnapshot struct {
	Revision string `json:"revision"`
	Config   Config `json:"config"`
}
type SaveProfileRequest struct {
	ExpectedRevision string  `json:"expectedRevision"`
	OriginalName     string  `json:"originalName"`
	Profile          Profile `json:"profile"`
}
type SaveSettingsRequest struct {
	ExpectedRevision string   `json:"expectedRevision"`
	Settings         Settings `json:"settings"`
}
type KnownHost struct {
	Host        string `toml:"host"`
	Port        int    `toml:"port"`
	Fingerprint string `toml:"fingerprint"`
	FirstSeen   int64  `toml:"first_seen_unix"`
	LastSeen    int64  `toml:"last_seen_unix"`
}
