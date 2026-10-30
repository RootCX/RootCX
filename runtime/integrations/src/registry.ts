// Imports are resolved only by build/qualification tools. Deployed workers contain one plugin.
import notion from './providers/notion';
import stripe from './providers/stripe';
import hubspot from './providers/hubspot';
import sheets from './providers/google-sheets';
import drive from './providers/google-drive';
import gmail from './providers/gmail';
import calendar from './providers/google-calendar';
import airtable from './providers/airtable';
import slack from './providers/slack';
import salesforce from './providers/salesforce';
import linkedinApiBuilderz from './providers/linkedin-api-builderz';
export const plugins = [notion, stripe, hubspot, sheets, drive, gmail, calendar, airtable, slack, salesforce, linkedinApiBuilderz];
