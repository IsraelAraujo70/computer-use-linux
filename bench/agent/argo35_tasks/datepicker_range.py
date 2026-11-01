ID = "a35-datepicker-range"
CATEGORY = 'date-picker'
PAGE = "argo35/datepicker_range.html"
PROMPT = 'Choose check-in 14/03/2027, check-out 02/04/2027, 2 adults, then click "Buscar".'
TARGET = {'checkin': '2027-03-14', 'checkout': '2027-04-02', 'adults': 2}
def check(state, root):
    return (isinstance(state, dict) and state == TARGET, "Expected exact requested state")
WRONG = [{'checkin': '2027-03-14', 'checkout': '2027-04-02', 'adults': 1}, {'checkin': '2027-03-14', 'checkout': '2027-04-03', 'adults': 2}, {'checkin': '2027-03-14', 'checkout': '', 'adults': 2}]
