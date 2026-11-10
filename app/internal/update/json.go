package update

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"unicode/utf8"
)

// 元数据不接受重复字段，避免同一份清单被不同消费者解释为不同身份。
func decodeJSON(data []byte, value any, strict bool) error {
	if !utf8.Valid(data) {
		return fmt.Errorf("更新JSON不是有效UTF-8")
	}
	d := json.NewDecoder(bytes.NewReader(data))
	d.UseNumber()
	var scan func(int) error
	scan = func(depth int) error {
		if depth > 32 {
			return fmt.Errorf("更新JSON嵌套过深")
		}
		token, err := d.Token()
		if err != nil {
			return err
		}
		opening, ok := token.(json.Delim)
		if !ok {
			return nil
		}
		switch opening {
		case '{':
			seen := make(map[string]bool)
			for d.More() {
				key, err := d.Token()
				if err != nil {
					return err
				}
				name, ok := key.(string)
				if !ok || seen[name] {
					return fmt.Errorf("更新JSON包含无效或重复字段")
				}
				seen[name] = true
				if err = scan(depth + 1); err != nil {
					return err
				}
			}
		case '[':
			for d.More() {
				if err := scan(depth + 1); err != nil {
					return err
				}
			}
		default:
			return fmt.Errorf("更新JSON结构无效")
		}
		_, err = d.Token()
		return err
	}
	if err := scan(0); err != nil {
		return err
	}
	if _, err := d.Token(); err != io.EOF {
		return fmt.Errorf("更新JSON包含尾随内容")
	}
	d = json.NewDecoder(bytes.NewReader(data))
	if strict {
		d.DisallowUnknownFields()
	}
	return d.Decode(value)
}
