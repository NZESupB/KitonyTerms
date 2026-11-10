package osauth

import "strings"

func accountParts(user, domain, currentName, currentDomain string) (string, string, bool) {
	if left, right, ok := strings.Cut(user, `\`); ok {
		return right, left, true
	}
	if strings.Contains(user, "@") {
		return user, "", false
	}
	if domain == "" {
		if strings.EqualFold(user, currentName) && currentDomain != "" {
			domain = currentDomain
		} else {
			domain = "."
		}
	}
	return user, domain, true
}

func helloUnavailable(code uint32) bool {
	return code == 0x80004002 || code == 0x80040154 || code == 0x80040111 || code == 0x80070032
}

func helloResult(result int32) error {
	switch result {
	case 0:
		return nil
	case 4:
		return ErrBusy
	case 6:
		return ErrCancelled
	default:
		return ErrFailed
	}
}
