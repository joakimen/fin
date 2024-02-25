package internal

func Ellipsis(s string, maxLen int) string {
	strLen := len(s)
	ellipsisLen := 4
	if strLen >= ellipsisLen && strLen > maxLen && maxLen >= ellipsisLen {
		return s[:(maxLen-ellipsisLen)] + " ..."
	}
	return s
}
