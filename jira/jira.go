package jira

import (
	"net/http"
	"strconv"
	"time"

	"github.com/joakimen/fin/config"
	"github.com/joakimen/fin/internal"
	"github.com/joakimen/fin/task"
)

type Time struct {
	time.Time
}

// SearchIssuesPayload represents the root structure of the Jira Search API response.
type SearchIssuesPayload struct {
	StartAt    int     `json:"startAt"`
	MaxResults int     `json:"maxResults"`
	Total      int     `json:"total"`
	Issues     []Issue `json:"issues"`
}

// Issue represents a single issue from the Jira Search API.
type Issue struct {
	ID     string `json:"id"`
	Key    string `json:"key"`
	Fields struct {
		Summary        string `json:"summary"`
		ResolutionDate Time   `json:"resolutiondate"`
		Creator        struct {
			EmailAddress string `json:"emailAddress"`
			DisplayName  string `json:"displayName"`
		} `json:"creator"`
	} `json:"fields"`
}

// UnmarshalJSON adds support for deserializing Jira's date format
func (jiraTime *Time) UnmarshalJSON(data []byte) (err error) {
	s := string(data)
	s = s[1 : len(s)-1]                      // remove leading/trailing quotes in the date string
	layout := "2006-01-02T15:04:05.000-0700" // "-0700" means "-hhmm", which is what Jira returns

	t, err := time.Parse(layout, s)
	if err != nil {
		return err
	}
	jiraTime.Time = t
	return nil
}

func deserialize(body []byte) SearchIssuesPayload {
	return internal.Deserialize[SearchIssuesPayload](body)
}

// searchIssues queries the Jira Search API for issues that are resolved
// and assigned to the current user.
func searchIssues(cfg *config.Config) []byte {
	apiURL := cfg.Jira.APIHost + "/rest/api/3/search"
	req := internal.NewRequest(http.MethodGet, apiURL)
	req.SetBasicAuth(cfg.Jira.APIUser, cfg.Jira.APIToken)
	req.Header.Set("Accept", "application/json")

	queryParams := req.URL.Query()
	startOfDay := strconv.Itoa(int(time.Since(cfg.StartDate).Hours() / 24))
	queryParams.Add("jql", "assignee = currentUser() AND status = \"Done\" AND resolutiondate >= startOfDay(\"-"+startOfDay+"\")")
	queryParams.Add("fields", "summary,resolutiondate,creator.emailAddress,creator.displayName")
	req.URL.RawQuery = queryParams.Encode()

	client := internal.NewClient()
	resp := internal.DoRequest(client, req)

	return resp
}

func toTask(issue *Issue) task.Task {
	return task.Task{
		Source:      task.Jira,
		CompletedAt: issue.Fields.ResolutionDate.Time,
		Title:       issue.Fields.Summary,
	}
}

func toTasks(issues *[]Issue) []task.Task {
	tasks := make([]task.Task, 0, len(*issues))
	for _, activity := range *issues {
		tasks = append(tasks, toTask(&activity))
	}
	return tasks
}

// GetCompletedTasks queries Jira for issues that are resolved and assigned to the current user,
// and returns them as a generic Task slice.
func GetCompletedTasks(cfg *config.Config) []task.Task {
	resp := searchIssues(cfg)
	if cfg.SaveTestData {
		internal.WriteTestData("jira.json", resp)
	}
	searchIssuesPayload := deserialize(resp)
	return toTasks(&searchIssuesPayload.Issues)
}
