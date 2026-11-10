package update

func sameInstallPath(first, second string) bool {
	left, err := canonicalUpdatePath(first)
	if err != nil {
		return false
	}
	right, err := canonicalUpdatePath(second)
	return err == nil && left == right
}
