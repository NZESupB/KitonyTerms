package main

import (
	"context"

	"kitonyterms/internal/buildinputs"
)

const maxBuildInputFileBytes = buildinputs.MaxFileBytes
const maxBuildInputNodes = buildinputs.MaxNodes
const maxBuildInputDepth = buildinputs.MaxDepth

var windowsResources = []string{"build/windows/candidate.ico", "build/windows/wails.exe.manifest"}

func copyBuildInputs(ctx context.Context, project, target string) (map[string]string, error) {
	return buildinputs.Copy(ctx, project, target, windowsResources)
}

func verifyBuildInputs(ctx context.Context, project string, expected map[string]string) error {
	return buildinputs.Verify(ctx, project, windowsResources, expected)
}

func scanBuildInputs(ctx context.Context, project string, consume func(string, []byte) error) (map[string]string, error) {
	return buildinputs.Scan(ctx, project, windowsResources, consume)
}

func pathsOverlap(first, second string) bool {
	return buildinputs.Overlap(first, second)
}
