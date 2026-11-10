package model

import (
	"math"
	"strings"
	"testing"
)

func TestSettingsValidateBoundariesAndLiteralNames(t *testing.T) {
	valid := DefaultSettings()
	valid.Language = "english"
	valid.UIFontFamily = "字体 (UI), Regular; \"Quoted\""
	valid.Editors = []Editor{{Name: "本地编辑器", Command: `code --wait "{file}"`}}
	if err := valid.Validate(); err != nil {
		t.Fatal("合法字体/编辑器设置被拒绝", err)
	}
	cases := []struct {
		name   string
		change func(*Settings)
	}{
		{"font-nan", func(s *Settings) { s.FontSize = math.NaN() }},
		{"scale-inf", func(s *Settings) { s.UIFontScale = math.Inf(1) }},
		{"language", func(s *Settings) { s.Language = "unknown" }},
		{"cursor", func(s *Settings) { s.CursorStyle = "unknown" }},
		{"color-empty", func(s *Settings) { s.AccentColor = "" }},
		{"color-short", func(s *Settings) { s.AccentColor = "#fff" }},
		{"color-injection", func(s *Settings) { s.AccentColor = "red; color: red" }},
		{"color-nonhex", func(s *Settings) { s.AccentColor = "#gggggg" }},
		{"font-empty", func(s *Settings) { s.FontFamily = " " }},
		{"font-newline", func(s *Settings) { s.UIFontFamily = "A\nB" }},
		{"font-bound", func(s *Settings) { s.UIFontFamily = strings.Repeat("x", 513) }},
		{"triggers-count", func(s *Settings) { s.TriggerHighlights = make([]string, 65) }},
		{"trigger-control", func(s *Settings) { s.TriggerHighlights = []string{"\x00"} }},
		{"editor-count", func(s *Settings) { s.Editors = make([]Editor, 65) }},
		{"editor-duplicate", func(s *Settings) { s.Editors = []Editor{{Name: "A", Command: "code"}, {Name: "B", Command: " code "}} }},
		{"editor-empty", func(s *Settings) { s.Editors = []Editor{{Name: "A", Command: " "}} }},
		{"editor-control", func(s *Settings) { s.DefaultEditor = "code\x00 --wait" }},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			settings := DefaultSettings()
			tc.change(&settings)
			if settings.Validate() == nil {
				t.Fatal("无效设置被接受")
			}
		})
	}
}
