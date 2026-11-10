package shellintegration

import (
	"strconv"
	"strings"
)

// 仅观察目录协议和 DEC 备用屏模式；OSC/控制字符串有界且不解释其正文中的 CSI。
type controlParser struct {
	state   byte
	payload []byte
	discard bool
	capture bool
	alt     bool
}

func (p *controlParser) feed(data []byte, osc func(string)) {
	for _, b := range data {
		switch p.state {
		case 0:
			if b == 0x1b {
				p.state = 'e'
			}
		case 'e':
			p.payload = p.payload[:0]
			p.discard = false
			switch b {
			case '[':
				p.state = 'c'
			case ']':
				p.state = 'o'
			case 'P', 'X', '^', '_':
				p.state = 's'
			case 'c':
				p.alt = false
				p.state = 0
			case 0x1b:
				p.state = 'e'
			default:
				p.state = 0
			}
		case 'c':
			if b == 0x1b {
				p.state = 'e'
				continue
			}
			if b >= 0x40 && b <= 0x7e {
				if !p.discard && (b == 'h' || b == 'l') && len(p.payload) > 0 && p.payload[0] == '?' {
					for _, value := range strings.Split(string(p.payload[1:]), ";") {
						mode, err := strconv.Atoi(value)
						if err == nil && (mode == 47 || mode == 1047 || mode == 1049) {
							p.alt = b == 'h'
						}
					}
				}
				p.state = 0
			} else if len(p.payload) < 128 {
				p.payload = append(p.payload, b)
			} else {
				p.discard = true
			}
		case 'o':
			switch b {
			case 0x07:
				p.finish(osc)
			case 0x1b:
				p.state = 'O'
			default:
				if p.capture && !p.discard {
					if len(p.payload) < 4096 {
						p.payload = append(p.payload, b)
					} else {
						p.discard = true
						p.payload = p.payload[:0]
					}
				}
			}
		case 'O':
			if b == '\\' {
				p.finish(osc)
			} else {
				p.payload = p.payload[:0]
				p.discard = false
				p.state = 'e'
				// 非ST的ESC开始新控制序列。
				p.feed([]byte{b}, osc)
			}
		case 's':
			if b == 0x1b {
				p.state = 'S'
			}
		case 'S':
			if b == '\\' {
				p.state = 0
			} else if b != 0x1b {
				p.state = 's'
			}
		}
	}
}
func (p *controlParser) finish(osc func(string)) {
	if p.capture && !p.discard && osc != nil {
		osc(string(p.payload))
	}
	p.payload = p.payload[:0]
	p.state = 0
	p.discard = false
}

func decodeCwd(payload string) string {
	rest, ok := strings.CutPrefix(payload, "7;file://")
	if !ok {
		return ""
	}
	index := strings.IndexByte(rest, '/')
	if index < 0 {
		return ""
	}
	input := rest[index:]
	decoded := make([]byte, 0, len(input))
	for i := 0; i < len(input); i++ {
		if input[i] == '%' && i+2 < len(input) {
			high, low := hexDigit(input[i+1]), hexDigit(input[i+2])
			if high >= 0 && low >= 0 {
				decoded = append(decoded, byte(high*16+low))
				i += 2
				continue
			}
		}
		decoded = append(decoded, input[i])
	}
	result := strings.ToValidUTF8(string(decoded), "�")
	if strings.ContainsAny(result, "\x00\r\n") {
		return ""
	}
	return result
}
func hexDigit(b byte) int {
	switch {
	case b >= '0' && b <= '9':
		return int(b - '0')
	case b >= 'A' && b <= 'F':
		return int(b - 'A' + 10)
	case b >= 'a' && b <= 'f':
		return int(b - 'a' + 10)
	default:
		return -1
	}
}
