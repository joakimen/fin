package gcal

import (
	"context"
	"fmt"
	"github.com/joakimen/fin/config"
	"github.com/joakimen/fin/task"
	"google.golang.org/api/calendar/v3"
	"google.golang.org/api/option"
	"log"
	"log/slog"
	"time"
)

type jiraTime struct {
	time.Time
}

func toTask(event *calendar.Event) task.Task {
	startTime, err := time.Parse(time.RFC3339, event.Start.DateTime)
	if err != nil {
		panic(fmt.Errorf("error parsing event start time: %v", err))
	}
	endTime, err := time.Parse(time.RFC3339, event.End.DateTime)
	if err != nil {
		panic(fmt.Errorf("error parsing event end time: %v", err))
	}

	return task.Task{
		Source:      task.GCal,
		StartsAt:    startTime,
		CompletedAt: endTime,
		Title:       event.Summary,
	}
}

func toTasks(events []*calendar.Event) []task.Task {
	tasks := make([]task.Task, 0, len(events))
	for _, event := range events {
		tasks = append(tasks, toTask(event))
	}
	return tasks
}

// GetCompletedTasks queries Jira for issues that are resolved and assigned to the current user,
// and returns them as a generic Task slice.
func GetCompletedTasks(cfg *config.Config) []task.Task {
	slog.Debug("querying gcal events")

	slog.Debug("creating calendar service")
	srv, err := calendar.NewService(context.TODO(), option.WithCredentialsFile("creds.json"))
	if err != nil {
		log.Fatalf("Unable to create Calendar service: %v", err)
	}

	// list calendars
	calendarList, err := srv.CalendarList.List().Do()
	if err != nil {
		log.Fatalf("Unable to retrieve calendar list: %v", err)
	}
	// print them
	for _, item := range calendarList.Items {
		fmt.Printf("%s (%s)\n", item.Summary, item.Id)
	}

	// Define the time range for which you want to retrieve events (last 7 days)
	now := time.Now()
	timeMin := now.AddDate(0, 0, -3).Format(time.RFC3339)
	timeMax := now.Format(time.RFC3339)

	// Query events
	slog.Debug("querying gcal events")
	events, err := srv.Events.List(cfg.GCal.CalendarID).ShowDeleted(false).
		SingleEvents(true).TimeMin(timeMin).TimeMax(timeMax).OrderBy("startTime").Do()

	if err != nil {
		log.Fatalf("unable to retrieve events: %v", err)
	}

	slog.Debug("received response from gcal")
	// Print the summary of each event
	fmt.Println("Events:")
	for _, item := range events.Items {
		summary := item.Summary
		startTime := item.Start.DateTime
		endTime := item.End.DateTime
		fmt.Printf("%s (%s - %s)\n", summary, startTime, endTime)
	}

	slog.Debug(fmt.Sprintf("returning %d past gcal events", len(events.Items)))
	return toTasks(events.Items)
}
