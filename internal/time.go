package internal

import "time"

func GetDaysBack(numberOfDaysBack int) time.Time {
	years := 0
	months := 0
	return time.Now().AddDate(years, months, -numberOfDaysBack)
}

func GetStartOfDay(t time.Time) time.Time {
	return t.Truncate(24 * time.Hour)
}
